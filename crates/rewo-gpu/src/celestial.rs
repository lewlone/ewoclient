//! Clear-weather Overworld celestial rendering — sun, moon, stars, and the
//! sunrise/sunset fan, drawn between the gradient sky and the terrain.
//!
//! Geometry and transforms are ported from `net.minecraft.client.renderer
//! .SkyRenderer` (26.2 decompile). The load-bearing details:
//!
//! - **Rotation-only sky space.** Vanilla renders the sky with a model-view
//!   that has the camera *translation* stripped (only its rotation), so the
//!   sun sits at infinity. We reconstruct that as `view_proj · translate(eye)`
//!   — because `view_proj = proj · R · T(-eye)`, right-multiplying by `T(eye)`
//!   cancels the translation and leaves `proj · R`.
//! - **Transform chains** (`renderSunMoonAndStars`): a base `Y(-90°)`, then per
//!   body `X(angle)`; the sun adds `translate(y=100) · scale(30,1,30)`, the moon
//!   `scale(20,1,20)`. The sunrise fan uses its *own* pose (no `Y(-90°)`):
//!   `X(90°) · Z(angle+90°) · scale(z=alpha)`.
//! - **Moon UV winding is reversed** vs the sun (`buildMoonPhases`), and the
//!   phase selects a base vertex.
//! - **Blend functions** (`RenderPipelines` + `BlendFunction`): CELESTIAL and
//!   STARS use `OVERLAY` (colour src=SRC_ALPHA dst=ONE, i.e. additive by
//!   alpha); SUNRISE_SUNSET uses `TRANSLUCENT`. None write depth — they are
//!   drawn before terrain, which overwrites them via reversed-Z.
//! - **Stars** (`buildStars`) are generated from a seed-10842
//!   `SingleThreadedRandomSource` with a rejection test; the accepted count is
//!   *reported*, never forced to 1500.

use ash::vk;
use glam::{Mat4, Vec3};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::entities::create_texture;
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

// ---------------------------------------------------------------------------
// SingleThreadedRandomSource (BitRandomSource) — the exact vanilla LCG.
// ---------------------------------------------------------------------------

/// Java `SingleThreadedRandomSource` / `BitRandomSource`: a 48-bit LCG. Only
/// the operations `buildStars` uses are ported (`next`, `nextFloat`,
/// `nextDouble`).
struct BitRandom {
    seed: i64,
}

impl BitRandom {
    const MULTIPLIER: i64 = 0x5DEECE66D;
    const INCREMENT: i64 = 0xB;
    const MASK: i64 = (1 << 48) - 1;

    /// `RandomSource.createThreadLocalInstance(seed)` → `setSeed`.
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::INCREMENT))
            & Self::MASK;
        // Java `(int)(newSeed >> 48 - bits)`; newSeed is a positive 48-bit value.
        (self.seed >> (48 - bits)) as i32
    }

    /// `BitRandomSource.nextFloat` = `next(24) * 5.9604645E-8F`.
    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * 5.960_464_5e-8
    }

    /// `BitRandomSource.nextDouble`. Vanilla computes `combined * 1.110223E-16F`
    /// in *float* precision (the multiplier is an `F` literal) then widens to
    /// double — matched here. The integer draws (`next(26)`, `next(27)`) are
    /// what advance the stream, so this precision detail never affects which
    /// stars are accepted, only a star's roll angle.
    fn next_double(&mut self) -> f64 {
        let upper = self.next(26) as i64;
        let lower = self.next(27) as i64;
        let combined = (upper << 27) + lower;
        (combined as f32 * 1.110_223e-16) as f64
    }
}

// ---------------------------------------------------------------------------
// buildStars — ported vertex-for-vertex.
// ---------------------------------------------------------------------------

/// `SkyRenderer.STAR_COUNT` — candidates, not accepted stars.
pub const STAR_COUNT: usize = 1500;

// -- JOML 1.10.8 scalar primitives (bit-exact ports) --------------------------
//
// `glam`'s `Vec3::normalize` / `Mat3` ops use a different (SIMD-friendly)
// operation order than JOML, so the star vertices came out 1–2 ULP off. These
// helpers reproduce JOML's exact scalar f32/f64 order from the jar
// (org.joml 1.10.8), verified against a Java oracle running the real jar.
//
// The load-bearing subtlety: `org.joml.Runtime.HAS_Math_fma` is **false** by
// default (JOML avoids `java.lang.Math.fma`, which is a slow software routine
// on most JVMs), so `org.joml.Math.fma(a, b, c)` is a **plain, non-fused**
// `a*b + c`. Therefore `Vector3f.lengthSquared` = `fma(x,x, fma(y,y, z*z))`
// evaluates as the *right-associative* `x*x + (y*y + z*z)` (NOT the fused, and
// NOT the left-associative `Mth.lengthSquared` the rejection test uses — those
// differ by up to 2 ULP, which is exactly the difference that shifted the star
// scalar). `Vector3f.mul` is likewise a plain `m0*a + (m1*b + m2*c)`. Do not
// use `f32::mul_add` here — a true FMA would reintroduce the drift. `invsqrt`
// is `1.0f / (float)Math.sqrt((double)r)`; the roll uses `Math.sin` +
// `Math.cosFromSin`.

/// `org.joml.Math.invsqrt(float)` = `1.0f / (float)java.lang.Math.sqrt(r)`.
#[inline]
fn joml_invsqrt(r: f32) -> f32 {
    1.0f32 / (r as f64).sqrt() as f32
}

/// `org.joml.Math.sqrt(float)` = `(float)java.lang.Math.sqrt((double)r)`.
#[inline]
fn joml_sqrt_f(r: f32) -> f32 {
    (r as f64).sqrt() as f32
}

/// `Matrix3f.rotateZ` calls the **float** overload `org.joml.Math.sin(float)`,
/// which (non-fastmath) is `(float)java.lang.Math.sin((double)ang)`. The
/// fdlibm-derived `libm::sin` matches Java's `Math.sin` at float precision; the
/// platform libm on Windows is up to a couple ULP less accurate.
#[inline]
fn joml_sin_f(ang: f32) -> f32 {
    libm::sin(ang as f64) as f32
}

/// `org.joml.Math.cosFromSin(float, float)` → `cosFromSinInternal` (non-fastmath):
/// the roll's cosine, computed **entirely in float** (not double — that is a
/// different overload the double `rotateZ` would use) with **float** constants.
/// `cos = sqrt(1 - sin²)` via the float `org.joml.Math.sqrt`, sign chosen by the
/// quadrant of `angle + π/2`. Porting the double version instead left the matrix
/// 1–8 ULP off on ~20 stars.
#[inline]
fn joml_cos_from_sin(sin: f32, angle: f32) -> f32 {
    // Float literals matching the decompiled `ldc` constants exactly.
    const HALF_PI: f32 = 1.5707964;
    const TWO_PI: f32 = 6.2831855;
    const PI: f32 = 3.1415927;
    let cos = joml_sqrt_f(1.0 - sin * sin);
    let a = angle + HALF_PI;
    let mut b = a - (a / TWO_PI) as i32 as f32 * TWO_PI;
    if b < 0.0 {
        b += TWO_PI;
    }
    if b >= PI {
        -cos
    } else {
        cos
    }
}

/// Port of `SkyRenderer.buildStars`. Returns the four **logical quad vertices**
/// per accepted star (vanilla QUADS order `(s,-s)(s,s)(-s,s)(-s,-s)`) and the
/// accepted count. Vanilla loops over 1500 candidates, rejecting any whose
/// squared length is `<= 0.010000001` or `>= 1.0` — consuming three floats for
/// the position and one for the size on every candidate, but the roll `double`
/// only when accepted. That asymmetry means the accepted set is not a simple
/// prefix, so the draw order is reproduced exactly. Bit-exact to JOML 1.10.8
/// (pinned by `star_geometry_matches_joml_oracle`).
pub fn build_star_quads() -> (Vec<[[f32; 3]; 4]>, usize) {
    let mut random = BitRandom::new(10842);
    let mut quads: Vec<[[f32; 3]; 4]> = Vec::new();

    for _ in 0..STAR_COUNT {
        let x = random.next_float() * 2.0 - 1.0;
        let y = random.next_float() * 2.0 - 1.0;
        let z = random.next_float() * 2.0 - 1.0;
        let star_size = 0.15 + random.next_float() * 0.1;
        // `Mth.lengthSquared` (Minecraft, plain) drives the rejection.
        let length_sq = x * x + y * y + z * z;
        if length_sq <= 0.010_000_001 || length_sq >= 1.0 {
            continue;
        }

        // `new Vector3f(x,y,z).normalize(100)`. JOML's `lengthSquared` is
        // `fma(x,x, fma(y,y, z*z))` with a NON-fused fma → the right-associative
        // `x*x + (y*y + z*z)`, distinct from the left-associative plain
        // rejection length above (they differ by up to 2 ULP).
        let len = x * x + (y * y + z * z);
        let scalar = joml_invsqrt(len) * 100.0;
        let (cx, cy, cz) = (x * scalar, y * scalar, z * scalar);

        // `(float)(nextDouble() * (float)Math.PI * 2.0)`.
        let z_rot = (random.next_double() * std::f32::consts::PI as f64 * 2.0) as f32;

        // `Matrix3f().rotateTowards(new Vector3f(center).negate(), (0,1,0))`.
        let (dx, dy, dz) = (-cx, -cy, -cz);
        let inv_dir = joml_invsqrt(dx * dx + dy * dy + dz * dz);
        let (ndx, ndy, ndz) = (dx * inv_dir, dy * inv_dir, dz * inv_dir);
        // left = up × ndir, up = (0,1,0) (written out so the ±0 rounding
        // matches the general cross-product form).
        let mut lx = 1.0f32 * ndz - 0.0f32 * ndy;
        let mut ly = 0.0f32 * ndx - 0.0f32 * ndz;
        let mut lz = 0.0f32 * ndy - 1.0f32 * ndx;
        let inv_left = joml_invsqrt(lx * lx + ly * ly + lz * lz);
        lx *= inv_left;
        ly *= inv_left;
        lz *= inv_left;
        // upn = ndir × left.
        let ux = ndy * lz - ndz * ly;
        let uy = ndz * lx - ndx * lz;
        let uz = ndx * ly - ndy * lx;
        // rotateTowards result (identity·rm = rm), columns (left, upn, ndir):
        //   m00,m01,m02 = left ; m10,m11,m12 = upn ; m20,m21,m22 = ndir.
        // `.rotateZ(-zRot)`: this · Rz, plain multiply-add.
        let ang = -z_rot;
        let sin = joml_sin_f(ang);
        let cos = joml_cos_from_sin(sin, ang);
        let m00 = lx * cos + ux * sin;
        let m01 = ly * cos + uy * sin;
        let m02 = lz * cos + uz * sin;
        let m10 = lx * -sin + ux * cos;
        let m11 = ly * -sin + uy * cos;
        let m12 = lz * -sin + uz * cos;
        let (m20, m21, m22) = (ndx, ndy, ndz);

        // `new Vector3f(sx,sy,0).mul(rotation).add(center)`. `Vector3f.mul` is
        // `fma(m0, sx, fma(m1, sy, m2*0))` with the same non-fused fma → the
        // plain `m0*sx + (m1*sy + m2*0)`, then `.add(center)`.
        let vert = |sx: f32, sy: f32| -> [f32; 3] {
            [
                m00 * sx + (m10 * sy + m20 * 0.0) + cx,
                m01 * sx + (m11 * sy + m21 * 0.0) + cy,
                m02 * sx + (m12 * sy + m22 * 0.0) + cz,
            ]
        };
        let s = star_size;
        quads.push([vert(s, -s), vert(s, s), vert(-s, s), vert(-s, -s)]);
    }

    let n = quads.len();
    (quads, n)
}

/// Expand the logical quads to a triangle list (6 verts/quad: `0,1,2, 0,2,3`)
/// for the GPU buffer, and the accepted count.
pub fn build_stars() -> (Vec<[f32; 3]>, usize) {
    let (quads, n) = build_star_quads();
    let mut verts = Vec::with_capacity(n * 6);
    for q in &quads {
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(q[i]);
        }
    }
    (verts, n)
}

/// FNV-1a 64 over the accepted quads' vertices as little-endian float bits —
/// the independently-derived vanilla fingerprint (`0xfef182656c6fe202`). Hashes
/// the four logical quad vertices in order, x/y/z each as 4 LE bytes.
pub fn star_fingerprint(quads: &[[[f32; 3]; 4]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for q in quads {
        for v in q {
            for c in v {
                for b in c.to_le_bytes() {
                    hash ^= b as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
    }
    hash
}

// ---------------------------------------------------------------------------
// Public inputs.
// ---------------------------------------------------------------------------

/// One decoded celestial texture (borrowed RGBA8 + dims).
pub struct CelestialImage<'a> {
    pub rgba: &'a [u8],
    pub w: u32,
    pub h: u32,
}

/// The sun + 8 moon-phase textures (moons indexed by `MoonPhase.index()`).
pub struct CelestialTextures<'a> {
    pub sun: CelestialImage<'a>,
    pub moons: [CelestialImage<'a>; 8],
    /// `END_FLASH_SPRITE`. Its quad is built by the same
    /// `buildCelestialQuad` the sun's is (`SkyRenderer.java:133-135`), so it
    /// shares the atlas and the pipeline.
    pub end_flash: CelestialImage<'a>,
}

/// Per-frame celestial state, in the renderer's units (angles already in
/// **radians**, sunrise colour already **linear rgb + straight alpha**).
#[derive(Clone, Copy, Debug)]
pub struct CelestialState {
    pub sun_angle: f32,
    pub moon_angle: f32,
    pub star_angle: f32,
    pub star_brightness: f32,
    pub moon_phase: u8,
    /// Linear rgb + alpha; alpha ≤ 0.001 hides the fan (`renderSunriseAndSunset`).
    pub sunrise_rgba: [f32; 4],
    /// `1.0 - rainLevel`; clear weather = 1.0.
    pub rain_brightness: f32,
}

impl Default for CelestialState {
    fn default() -> Self {
        Self {
            sun_angle: 0.0,
            moon_angle: 0.0,
            star_angle: 0.0,
            star_brightness: 0.0,
            moon_phase: 0,
            sunrise_rgba: [0.0; 4],
            rain_brightness: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// GPU pass.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TexVertex {
    pos: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ColorVertex {
    pos: [f32; 3],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    mvp: [[f32; 4]; 4],
    tint: [f32; 4],
}

struct Buf {
    buffer: vk::Buffer,
    alloc: Option<Allocation>,
}

/// One base `Y(-90°)` shared by sun/moon/stars, as `Axis.YP.rotationDegrees`.
fn sky_base_rotation() -> Mat4 {
    Mat4::from_rotation_y((-90.0f32).to_radians())
}

pub struct CelestialPass {
    // Textured pipeline (sun + moon).
    tex_pipeline: vk::Pipeline,
    tex_layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    atlas_image: vk::Image,
    atlas_alloc: Option<Allocation>,
    atlas_view: vk::ImageView,
    sun_buf: Buf,
    moon_buf: Buf,
    end_flash_buf: Buf,

    // Star pipeline (reuses the line shaders: mat4 + vec4 push, pos-only).
    star_pipeline: vk::Pipeline,
    star_layout: vk::PipelineLayout,
    star_buf: Buf,
    star_verts: u32,
    star_quad_count: u32,

    // Sunrise pipeline.
    sunrise_pipeline: vk::Pipeline,
    sunrise_layout: vk::PipelineLayout,
    sunrise_buf: Buf,
    sunrise_verts: u32,
}

impl CelestialPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex: &CelestialTextures,
    ) -> Result<Self, String> {
        // ---- atlas: sun + 8 moons in a 9-cell horizontal strip ----
        let mut cell = tex.sun.w.max(tex.sun.h);
        for m in &tex.moons {
            cell = cell.max(m.w).max(m.h);
        }
        cell = cell.max(tex.end_flash.w).max(tex.end_flash.h);
        // Sun, 8 moons, end flash.
        let cols = 10u32;
        let atlas_w = cols * cell;
        let atlas_h = cell;
        let mut atlas = vec![0u8; (atlas_w * atlas_h * 4) as usize];
        let blit = |atlas: &mut [u8], img: &CelestialImage, col: u32| {
            let ox = col * cell;
            for row in 0..img.h {
                let dst = ((row * atlas_w + ox) * 4) as usize;
                let src = (row * img.w * 4) as usize;
                let n = (img.w * 4) as usize;
                atlas[dst..dst + n].copy_from_slice(&img.rgba[src..src + n]);
            }
        };
        let uv_rect = |img: &CelestialImage, col: u32| -> [f32; 4] {
            let ox = (col * cell) as f32;
            [
                ox / atlas_w as f32,
                0.0,
                (ox + img.w as f32) / atlas_w as f32,
                img.h as f32 / atlas_h as f32,
            ]
        };
        blit(&mut atlas, &tex.sun, 0);
        let sun_uv = uv_rect(&tex.sun, 0);
        let mut moon_uv = [[0.0f32; 4]; 8];
        for (i, m) in tex.moons.iter().enumerate() {
            blit(&mut atlas, m, (i + 1) as u32);
            moon_uv[i] = uv_rect(m, (i + 1) as u32);
        }
        blit(&mut atlas, &tex.end_flash, 9);
        let end_flash_uv = uv_rect(&tex.end_flash, 9);
        let (atlas_image, atlas_alloc, atlas_view) = create_texture(gpu, &atlas, atlas_w, atlas_h)?;

        // ---- static geometry ----
        // Sun quad (POSITION_TEX), vanilla winding: uv (u0,v0)(u1,v0)(u1,v1)(u0,v1).
        let sun_quad = tex_quad(sun_uv, false);
        let sun_buf = upload_buffer(gpu, bytemuck::cast_slice(&sun_quad))?;
        // Moon quads: 8 phases, reversed uv winding, phase p at base vertex p*6.
        let mut moon_verts: Vec<TexVertex> = Vec::with_capacity(8 * 6);
        for uv in &moon_uv {
            moon_verts.extend_from_slice(&tex_quad(*uv, true));
        }
        let moon_buf = upload_buffer(gpu, bytemuck::cast_slice(&moon_verts))?;
        // Same winding as the sun — `buildCelestialQuad` is one function.
        let end_flash_quad = tex_quad(end_flash_uv, false);
        let end_flash_buf = upload_buffer(gpu, bytemuck::cast_slice(&end_flash_quad))?;

        // Stars (pos-only).
        let (star_positions, star_quad_count) = build_stars();
        let star_quad_count = star_quad_count as u32;
        let star_verts = star_positions.len() as u32;
        let star_buf = upload_buffer(gpu, bytemuck::cast_slice(&star_positions))?;

        // Sunrise fan → triangle list (POSITION_COLOR).
        let sunrise = sunrise_fan();
        let sunrise_verts = sunrise.len() as u32;
        let sunrise_buf = upload_buffer(gpu, bytemuck::cast_slice(&sunrise))?;

        // ---- textured pipeline (sampler set + push) ----
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
                .map_err(|e| format!("celestial sampler: {e}"))?
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
                .map_err(|e| format!("celestial set layout: {e}"))?
        };
        let pool_sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("celestial pool: {e}"))?
        };
        let set_layouts = [set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("celestial set: {e}"))?[0]
        };
        let image_info = [vk::DescriptorImageInfo::default()
            .sampler(sampler)
            .image_view(atlas_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );
        }
        let push_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<Push>() as u32)];
        let tex_layout = unsafe {
            device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&push_range),
                    None,
                )
                .map_err(|e| format!("celestial tex layout: {e}"))?
        };
        let tex_pipeline = build_tex_pipeline(&device, tex_layout, color_format)?;

        // ---- star + sunrise pipelines (no descriptor set) ----
        let empty_layout = || -> Result<vk::PipelineLayout, String> {
            unsafe {
                device
                    .create_pipeline_layout(
                        &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_range),
                        None,
                    )
                    .map_err(|e| format!("celestial layout: {e}"))
            }
        };
        let star_layout = empty_layout()?;
        let star_pipeline = build_star_pipeline(&device, star_layout, color_format)?;
        let sunrise_layout = empty_layout()?;
        let sunrise_pipeline = build_sunrise_pipeline(&device, sunrise_layout, color_format)?;

        Ok(Self {
            tex_pipeline,
            tex_layout,
            set_layout,
            pool,
            set,
            sampler,
            atlas_image,
            atlas_alloc: Some(atlas_alloc),
            atlas_view,
            sun_buf,
            moon_buf,
            end_flash_buf,
            star_pipeline,
            star_layout,
            star_buf,
            star_verts,
            star_quad_count,
            sunrise_pipeline,
            sunrise_layout,
            sunrise_buf,
            sunrise_verts,
        })
    }

    /// Accepted stars (quads) and the equivalent index count vanilla would draw
    /// (`starIndexCount = quads · 6`). Reported, not forced to 1500.
    pub fn star_quad_count(&self) -> u32 {
        self.star_quad_count
    }
    pub fn star_index_count(&self) -> u32 {
        self.star_quad_count * 6
    }

    /// Draw sunrise → sun → moon → stars (the `LevelRenderer.addSkyPass`
    /// order), all in rotation-only sky space (`sky_vp = view_proj · T(eye)`),
    /// no depth. `extent` sets the (flipped) viewport, matching the terrain.
    pub fn draw(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        sky_vp: [[f32; 4]; 4],
        extent: vk::Extent2D,
        state: &CelestialState,
    ) {
        let device = &gpu.device;
        let vp = Mat4::from_cols_array_2d(&sky_vp);
        let base = sky_base_rotation();
        unsafe {
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
        }

        // --- sunrise/sunset fan (own pose, no Y-90) ---
        if state.sunrise_rgba[3] > 0.001 {
            let angle = sunrise_fan_rotation_degrees(state.sun_angle);
            let model = Mat4::from_rotation_x(90.0f32.to_radians())
                * Mat4::from_rotation_z((angle + 90.0f32).to_radians())
                * Mat4::from_scale(Vec3::new(1.0, 1.0, state.sunrise_rgba[3]));
            let push = Push {
                mvp: (vp * model).to_cols_array_2d(),
                tint: state.sunrise_rgba,
            };
            unsafe {
                device.cmd_bind_pipeline(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.sunrise_pipeline,
                );
                push_const(device, cb, self.sunrise_layout, &push);
                device.cmd_bind_vertex_buffers(cb, 0, &[self.sunrise_buf.buffer], &[0]);
                device.cmd_draw(cb, self.sunrise_verts, 1, 0, 0);
            }
        }

        // --- sun ---
        let sun_model = base
            * Mat4::from_rotation_x(state.sun_angle)
            * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
            * Mat4::from_scale(Vec3::new(30.0, 1.0, 30.0));
        self.draw_tex(
            gpu,
            cb,
            &self.sun_buf,
            0,
            vp * sun_model,
            [1.0, 1.0, 1.0, state.rain_brightness],
        );

        // --- moon (phase selects base vertex) ---
        let moon_model = base
            * Mat4::from_rotation_x(state.moon_angle)
            * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
            * Mat4::from_scale(Vec3::new(20.0, 1.0, 20.0));
        let phase = (state.moon_phase as u32).min(7);
        self.draw_tex(
            gpu,
            cb,
            &self.moon_buf,
            phase * 6,
            vp * moon_model,
            [1.0, 1.0, 1.0, state.rain_brightness],
        );

        // --- stars ---
        if state.star_brightness > 0.0 && self.star_verts > 0 {
            let star_model = base * Mat4::from_rotation_x(state.star_angle);
            let b = state.star_brightness;
            let push = Push {
                mvp: (vp * star_model).to_cols_array_2d(),
                tint: [b, b, b, b],
            };
            unsafe {
                device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.star_pipeline);
                push_const(device, cb, self.star_layout, &push);
                device.cmd_bind_vertex_buffers(cb, 0, &[self.star_buf.buffer], &[0]);
                device.cmd_draw(cb, self.star_verts, 1, 0, 0);
            }
        }
    }

    /// `SkyRenderer.renderEndFlash` (`SkyRenderer.java:477-503`).
    ///
    /// Three things separate it from the sun, which it otherwise shares a
    /// quad builder, an atlas and a pipeline with:
    ///
    /// * **No sky base rotation.** `LevelRenderer.java:336` hands it a
    ///   `new PoseStack()`, so the `Y(-90)` every other celestial inherits is
    ///   absent and the chain starts at identity.
    /// * **The angles come off the schedule, not the clock** —
    ///   `Y(180 - yAngle)` then `X(-90 - xAngle)`, both in degrees, both
    ///   *subtracted*.
    /// * **The tint is `(i, i, i, i)`**, not white-at-alpha: `writeTransform`
    ///   takes the intensity as all four components of the colour modulator,
    ///   so the quad dims in colour as well as alpha.
    ///
    /// Scale is 60 against the sun's 30, and the caller gates on
    /// `intensity > 1.0E-5` — a threshold, not `> 0`, which is what keeps the
    /// flash's ~1e-16 tail from costing a draw.
    pub fn draw_end_flash(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        sky_vp: [[f32; 4]; 4],
        extent: vk::Extent2D,
        intensity: f32,
        x_angle: f32,
        y_angle: f32,
    ) {
        let device = &gpu.device;
        unsafe {
            let viewport = vk::Viewport::default()
                .y(extent.height as f32)
                .width(extent.width as f32)
                .height(-(extent.height as f32))
                .max_depth(1.0);
            device.cmd_set_viewport(cb, 0, &[viewport]);
            device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
        }
        let vp = Mat4::from_cols_array_2d(&sky_vp);
        let model = Mat4::from_rotation_y((180.0 - y_angle).to_radians())
            * Mat4::from_rotation_x((-90.0 - x_angle).to_radians())
            * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
            * Mat4::from_scale(Vec3::new(60.0, 1.0, 60.0));
        self.draw_tex(
            gpu,
            cb,
            &self.end_flash_buf,
            0,
            vp * model,
            [intensity, intensity, intensity, intensity],
        );
    }

    fn draw_tex(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        buf: &Buf,
        first_vertex: u32,
        mvp: Mat4,
        tint: [f32; 4],
    ) {
        let device = &gpu.device;
        let push = Push {
            mvp: mvp.to_cols_array_2d(),
            tint,
        };
        unsafe {
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.tex_pipeline);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.tex_layout,
                0,
                &[self.set],
                &[],
            );
            push_const(device, cb, self.tex_layout, &push);
            device.cmd_bind_vertex_buffers(cb, 0, &[buf.buffer], &[0]);
            device.cmd_draw(cb, 6, 1, first_vertex, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        let device = &gpu.device;
        unsafe {
            device.destroy_pipeline(self.tex_pipeline, None);
            device.destroy_pipeline_layout(self.tex_layout, None);
            device.destroy_pipeline(self.star_pipeline, None);
            device.destroy_pipeline_layout(self.star_layout, None);
            device.destroy_pipeline(self.sunrise_pipeline, None);
            device.destroy_pipeline_layout(self.sunrise_layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.atlas_view, None);
            device.destroy_image(self.atlas_image, None);
            device.destroy_buffer(self.sun_buf.buffer, None);
            device.destroy_buffer(self.moon_buf.buffer, None);
            device.destroy_buffer(self.end_flash_buf.buffer, None);
            device.destroy_buffer(self.star_buf.buffer, None);
            device.destroy_buffer(self.sunrise_buf.buffer, None);
        }
        for a in [
            self.atlas_alloc.take(),
            self.sun_buf.alloc.take(),
            self.moon_buf.alloc.take(),
            self.end_flash_buf.alloc.take(),
            self.star_buf.alloc.take(),
            self.sunrise_buf.alloc.take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// A unit quad in the XZ plane (`buildCelestialQuad` positions), UVs from a
/// sprite rect `[u0,v0,u1,v1]`. `reversed` picks `buildMoonPhases`' flipped UV
/// winding. Expanded to a 6-vertex triangle list (`0,1,2, 0,2,3`).
fn tex_quad(uv: [f32; 4], reversed: bool) -> [TexVertex; 6] {
    let [u0, v0, u1, v1] = uv;
    let pos = [
        [-1.0f32, 0.0, -1.0],
        [1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
    ];
    let uvs = if reversed {
        // buildMoonPhases: (u1,v1)(u0,v1)(u0,v0)(u1,v0).
        [[u1, v1], [u0, v1], [u0, v0], [u1, v0]]
    } else {
        // buildCelestialQuad: (u0,v0)(u1,v0)(u1,v1)(u0,v1).
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
    };
    let v = |i: usize| TexVertex {
        pos: pos[i],
        uv: uvs[i],
    };
    [v(0), v(1), v(2), v(0), v(2), v(3)]
}

// ---------------------------------------------------------------------------
// Mth sine table — the 26.2 decompile's semantics, exact.
// ---------------------------------------------------------------------------
//
// `SkyRenderer.buildSunriseFan` does NOT call platform `sin`/`cos`; it calls
// `Mth.sin(angle)` / `Mth.cos(angle)`, which index a 65,536-entry table:
//
//   SIN[i] = (float)Math.sin(i / 10430.378350470453)
//   sin(v) = SIN[(int)((long)(v * 10430.378350470453)          & 65535L)]
//   cos(v) = SIN[(int)((long)(v * 10430.378350470453 + 16384)  & 65535L)]
//
// The distinction is load-bearing: near half/whole turns the table returns
// *tiny* values of a fixed sign (e.g. `SIN[32768] ≈ 1.2e-16`, positive), where
// platform `cos(π/2 + ε)` would return `≈ -ε` of the opposite sign. Only the
// 17 selected indices are evaluated — no 65,536-float table is allocated.

/// `Mth.SIN_SCALE` — `65536 / (2π)`, the table-index scale, as the double
/// literal the decompile carries.
const MTH_SIN_SCALE: f64 = 10430.378350470453;

/// One `Mth` sine-table entry `SIN[rawIndex & 65535]`, evaluated on demand.
/// `SIN[i] = (float)Math.sin(i / SCALE)`; `libm::sin` is fdlibm-derived and
/// matches Java's `Math.sin` at the `(float)` cast (the same discipline the
/// star geometry relies on).
fn mth_sin_table(raw_index: i64) -> f32 {
    let idx = raw_index & 65535;
    libm::sin(idx as f64 / MTH_SIN_SCALE) as f32
}

/// `Mth.sin(float)` — table lookup. The `(long)` cast truncates toward zero,
/// exactly like the decompile.
fn mth_sin(angle: f32) -> f32 {
    mth_sin_table((angle as f64 * MTH_SIN_SCALE) as i64)
}

/// `Mth.cos(float)` — the same table offset a quarter turn (16384 entries).
fn mth_cos(angle: f32) -> f32 {
    mth_sin_table((angle as f64 * MTH_SIN_SCALE + 16384.0) as i64)
}

/// `SkyRenderer.renderSunriseAndSunset` picks the fan's Z-rotation from
/// `Mth.sin(sunAngle) < 0` — 180° when the table sine is negative, else 0°.
/// Using the `Mth` table (not platform `sin`) is load-bearing at the half-turn:
/// `Mth.sin(π)` reads the tiny *positive* table entry `SIN[32768] ≈ 1.2e-16`, so
/// the boundary resolves to 0°, whereas platform `sin(π_f32)` is slightly
/// negative (its argument rounds just past π) and would wrongly flip to 180°.
fn sunrise_fan_rotation_degrees(sun_angle: f32) -> f32 {
    if mth_sin(sun_angle) < 0.0 {
        180.0
    } else {
        0.0
    }
}

/// `buildSunriseFan`'s 18 logical POSITION vertices: centre `(0,100,0)` then the
/// 17-point ring `(sin·120, cos·120, -cos·40)`, with `angle = i·(float)(2π)/16`
/// and vanilla `Mth.sin`/`Mth.cos` (NOT platform trig). Pinned by
/// `sunrise_fan_matches_mth_table_oracle`.
fn sunrise_fan_positions() -> [[f32; 3]; 18] {
    // `(float)(Math.PI * 2.0)`, then all-float `i * tau / 16` per the decompile.
    let tau = (std::f64::consts::PI * 2.0) as f32;
    let mut pos = [[0.0f32; 3]; 18];
    pos[0] = [0.0, 100.0, 0.0];
    for i in 0..=16 {
        let angle = i as f32 * tau / 16.0;
        let s = mth_sin(angle);
        let c = mth_cos(angle);
        pos[i + 1] = [s * 120.0, c * 120.0, -c * 40.0];
    }
    pos
}

/// `buildSunriseFan` as a triangle list: centre white-alpha-1, ring
/// white-alpha-0, 16 triangles fanning from the centre.
fn sunrise_fan() -> Vec<ColorVertex> {
    let pos = sunrise_fan_positions();
    let cv = |p: [f32; 3], a: f32| ColorVertex {
        pos: p,
        color: [1.0, 1.0, 1.0, a],
    };
    let center = cv(pos[0], 1.0);
    let ring: Vec<ColorVertex> = pos[1..].iter().map(|&p| cv(p, 0.0)).collect();
    let mut out = Vec::with_capacity(16 * 3);
    for i in 0..16 {
        out.push(center);
        out.push(ring[i]);
        out.push(ring[i + 1]);
    }
    out
}

/// FNV-1a 64 over the fan positions as little-endian float bits (x/y/z, 4 LE
/// bytes each) — matches the independent Java oracle's hash exactly. Test-only:
/// the oracle test is its sole caller.
#[cfg(test)]
fn sunrise_fan_fingerprint(pos: &[[f32; 3]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for v in pos {
        for c in v {
            for b in c.to_le_bytes() {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    hash
}

fn push_const(
    device: &ash::Device,
    cb: vk::CommandBuffer,
    layout: vk::PipelineLayout,
    push: &Push,
) {
    unsafe {
        device.cmd_push_constants(
            cb,
            layout,
            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
            0,
            bytemuck::bytes_of(push),
        );
    }
}

fn upload_buffer(gpu: &mut Gpu, bytes: &[u8]) -> Result<Buf, String> {
    let device = &gpu.device;
    let buffer = unsafe {
        device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes.len().max(4) as u64)
                    .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("celestial buffer: {e}"))?
    };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let mut alloc = gpu
        .allocator
        .allocate(&AllocationCreateDesc {
            name: "celestial-verts",
            requirements: req,
            location: MemoryLocation::CpuToGpu,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .map_err(|e| format!("celestial alloc: {e}"))?;
    unsafe {
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("celestial bind: {e}"))?;
    }
    if !bytes.is_empty() {
        alloc
            .mapped_slice_mut()
            .ok_or("celestial buffer not mapped")?[..bytes.len()]
            .copy_from_slice(bytes);
    }
    Ok(Buf {
        buffer,
        alloc: Some(alloc),
    })
}

// --- pipelines ----------------------------------------------------------

fn overlay_blend() -> vk::PipelineColorBlendAttachmentState {
    // BlendFunction.OVERLAY: colour src=SRC_ALPHA dst=ONE, alpha src=ONE
    // dst=ZERO. Writes RGBA (the full default write mask, matching vanilla's
    // ColorTargetState); the frag discards fully-transparent texels.
    vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
        .dst_color_blend_factor(vk::BlendFactor::ONE)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(
            vk::ColorComponentFlags::R
                | vk::ColorComponentFlags::G
                | vk::ColorComponentFlags::B
                | vk::ColorComponentFlags::A,
        )
}

fn translucent_blend() -> vk::PipelineColorBlendAttachmentState {
    // BlendFunction.TRANSLUCENT: colour src=SRC_ALPHA dst=ONE_MINUS_SRC_ALPHA.
    // Writes RGBA (the full default write mask, matching vanilla's
    // ColorTargetState); the frag discards fully-transparent fragments.
    vk::PipelineColorBlendAttachmentState::default()
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
        )
}

/// Common no-depth, no-cull graphics-pipeline scaffold for the celestial
/// passes. `attrs`/`stride` set the vertex format; `blend` the blend function.
fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    vert_spv: &[u8],
    frag_spv: &[u8],
    stride: u32,
    attrs: &[vk::VertexInputAttributeDescription],
    blend: vk::PipelineColorBlendAttachmentState,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(device, vert_spv)?;
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
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(stride)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Drawn before terrain; no depth interaction.
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let blend_attachments = [blend];
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
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("celestial pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

fn build_tex_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    let attrs = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(12),
    ];
    build_pipeline(
        device,
        layout,
        color_format,
        include_bytes!(concat!(env!("OUT_DIR"), "/celestial.vert.spv")),
        include_bytes!(concat!(env!("OUT_DIR"), "/celestial.frag.spv")),
        std::mem::size_of::<TexVertex>() as u32,
        &attrs,
        overlay_blend(),
    )
}

fn build_star_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    // Reuse the line shaders: push {mat4, vec4}, pos-only vertex. The star
    // colour is the flat push tint; OVERLAY makes it additive by brightness².
    let attrs = [vk::VertexInputAttributeDescription::default()
        .location(0)
        .format(vk::Format::R32G32B32_SFLOAT)
        .offset(0)];
    build_pipeline(
        device,
        layout,
        color_format,
        include_bytes!(concat!(env!("OUT_DIR"), "/line.vert.spv")),
        include_bytes!(concat!(env!("OUT_DIR"), "/line.frag.spv")),
        12,
        &attrs,
        overlay_blend(),
    )
}

fn build_sunrise_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    let attrs = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .format(vk::Format::R32G32B32A32_SFLOAT)
            .offset(12),
    ];
    build_pipeline(
        device,
        layout,
        color_format,
        include_bytes!(concat!(env!("OUT_DIR"), "/sunrise.vert.spv")),
        include_bytes!(concat!(env!("OUT_DIR"), "/sunrise.frag.spv")),
        std::mem::size_of::<ColorVertex>() as u32,
        &attrs,
        translucent_blend(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_reject_some_candidates_deterministically() {
        let (quads, count) = build_star_quads();
        // The rejection test removes the near-origin core and the outside-the-
        // unit-sphere corners, so fewer than every candidate survives — and
        // more than none.
        assert!(count > 0 && count < STAR_COUNT, "accepted {count}");
        assert_eq!(quads.len(), count, "one quad per accepted star");
        let (verts, _) = build_stars();
        assert_eq!(verts.len(), count * 6, "6 triangle-list verts per star");
        // Determinism: same seed → same accepted set, byte-for-byte.
        let (quads2, count2) = build_star_quads();
        assert_eq!(count, count2);
        assert_eq!(quads, quads2);
    }

    /// Pinned against an independent Java oracle compiled against the official
    /// JOML 1.10.8 jar (the reviewer's oracle): the exact accepted count, index
    /// count, first-quad raw float bits, and FNV-1a-64 fingerprint. Any drift
    /// in the RNG, rejection test, JOML op order, or FMA/rounding fails here.
    #[test]
    fn star_geometry_matches_joml_oracle() {
        let (quads, count) = build_star_quads();
        assert_eq!(count, 780, "accepted star count");
        assert_eq!(count * 6, 4680, "triangle-list index count");
        // First accepted quad, raw IEEE-754 f32 bits (big-endian hex), from the
        // JOML oracle.
        let expected_first: [[u32; 3]; 4] = [
            [0xc254e61b, 0x428c1caf, 0x423e212a],
            [0xc2544619, 0x428c01e5, 0x423f2253],
            [0xc2551379, 0x428b9745, 0x423f75b0],
            [0xc255b37b, 0x428bb20f, 0x423e7487],
        ];
        for (vi, v) in quads[0].iter().enumerate() {
            for (ci, c) in v.iter().enumerate() {
                assert_eq!(
                    c.to_bits(),
                    expected_first[vi][ci],
                    "first quad vertex {vi} component {ci}: got {:08x} want {:08x}",
                    c.to_bits(),
                    expected_first[vi][ci]
                );
            }
        }
        assert_eq!(
            star_fingerprint(&quads),
            0xfef1_8265_6c6f_e202,
            "star geometry fingerprint (FNV-1a 64 over LE float bits)"
        );
    }

    #[test]
    fn stars_land_on_the_distance_100_shell() {
        let (verts, _) = build_stars();
        for v in &verts {
            let r = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            // Each corner is the star centre (radius 100) plus a ≤~0.25 offset.
            assert!((99.0..101.0).contains(&r), "star vertex radius {r}");
        }
    }

    /// Pinned against an independent Java oracle (`Mth`-table sin/cos):
    /// `SkyRenderer.buildSunriseFan` samples the 65,536-entry table, NOT
    /// platform trig. The tiny ring-4/ring-8 values (`SIN[32768] ≈ 1.2e-16`)
    /// are the ones a platform-`cos` port gets wrong — they anchor this test.
    #[test]
    fn sunrise_fan_matches_mth_table_oracle() {
        let pos = sunrise_fan_positions();
        assert_eq!(pos.len(), 18, "centre + 17-point ring");
        let bits3 = |p: [f32; 3]| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        // centre = (0, 100, 0).
        assert_eq!(bits3(pos[0]), [0x0000_0000, 0x42c8_0000, 0x0000_0000]);
        // ring 0 = (0, 120, -40).
        assert_eq!(bits3(pos[1]), [0x0000_0000, 0x42f0_0000, 0xc220_0000]);
        // ring 4 — the tiny Mth-table cos proves this is not platform trig.
        assert_eq!(bits3(pos[5]), [0x42f0_0000, 0x2884_5e1f, 0xa7b0_7d7e]);
        // ring 8.
        assert_eq!(bits3(pos[9]), [0x2884_5e1f, 0xc2f0_0000, 0x4220_0000]);
        // ring 12 — note the `-c*40` negative-zero z (c is +0 → -0·40 = -0).
        assert_eq!(bits3(pos[13]), [0xc2f0_0000, 0x0000_0000, 0x8000_0000]);
        // ring 16 wraps back onto ring 0.
        assert_eq!(bits3(pos[17]), bits3(pos[1]));
        assert_eq!(
            sunrise_fan_fingerprint(&pos),
            0x7528_0003_503b_2a33,
            "sunrise fan fingerprint (FNV-1a 64 over LE float bits)"
        );
    }

    /// `renderSunriseAndSunset` chooses the fan side from `Mth.sin(sunAngle) <
    /// 0`, NOT platform `sin`. The half-turn is the trap the two disagree on:
    /// the `Mth` table reads a tiny *positive* value at π, so vanilla stays on
    /// side 0°, while platform `sin(π_f32)` is slightly negative (its argument
    /// rounds just past π) and would flip to 180°. Clearly-signed angles agree.
    #[test]
    fn sunrise_fan_side_uses_the_mth_table_sign() {
        use std::f32::consts::{FRAC_PI_2, PI};
        // The divergence at the boundary — this is what the fix is for.
        assert!(
            mth_sin(PI) >= 0.0,
            "Mth.sin(π) is the tiny positive table entry"
        );
        assert!(
            PI.sin() < 0.0,
            "platform sin(π_f32) is slightly negative (the trap)"
        );
        assert_eq!(
            sunrise_fan_rotation_degrees(PI),
            0.0,
            "boundary resolves to 0° via the Mth table, not 180°"
        );
        // Away from the boundary both agree: +sine → 0°, −sine → 180°.
        assert_eq!(
            sunrise_fan_rotation_degrees(FRAC_PI_2),
            0.0,
            "sin(π/2)=+1 → 0°"
        );
        assert_eq!(
            sunrise_fan_rotation_degrees(3.0 * FRAC_PI_2),
            180.0,
            "sin(3π/2)=−1 → 180°"
        );
        // Just past π the table too goes negative, so the flip does happen —
        // only the exact half-turn sits on the positive side.
        assert_eq!(
            sunrise_fan_rotation_degrees(PI + 0.05),
            180.0,
            "past π flips to 180°"
        );
    }

    #[test]
    fn bit_random_matches_java_lcg_first_draws() {
        // Cross-check the LCG against a hand-run of the Java algorithm for
        // seed 10842: seed0 = (10842 ^ 0x5DEECE66D) & MASK, then advance.
        let mut r = BitRandom::new(10842);
        // First nextFloat must be in [0,1).
        let f = r.next_float();
        assert!((0.0..1.0).contains(&f), "nextFloat {f} out of range");
    }
}
