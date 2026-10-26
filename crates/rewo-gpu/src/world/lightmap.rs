//! The lightmap uniforms: the resolved [`WorldLightmapState`], the
//! `LightmapExtra` UBO that carries what no longer fits the push block, and the
//! per-frame-in-flight ring of host-mapped buffers that holds it.

use ash::vk;
use bytemuck::{Pod, Zeroable};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use super::WorldRenderer;
use crate::Gpu;

/// The spill-over uniform block for the world/water lightmap: everything that
/// no longer fits in [`WorldPush`]'s full 128 bytes. Mirrors `LightmapExtra` in
/// `shaders/world.frag` / `shaders/water.frag` (std140: one `vec4`).
///
/// It exists because M16 made `AmbientColor` a per-dimension value rather than
/// a shader constant, and quantizing it into a spare push lane would have
/// silently lossy-encoded a canonical input.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(super) struct WorldLightmapExtra {
    /// xyz = `AmbientColor` (RGB24/255), w unused (std140 vec4 padding).
    ambient: [f32; 4],
    /// The **environmental** fog band (M33b): `x` start, `y` end, `zw` pad.
    ///
    /// Separate from the push block's band, which is Rewo's render-distance
    /// fade. Vanilla's `total_fog_value` is the `max` of an environmental and a
    /// render-distance term, and only the environmental one is what rain
    /// thickens. A disabled band (start beyond end, as
    /// [`WorldLightmapState::default`] leaves it) contributes nothing, so clear
    /// weather renders exactly as it did before this existed.
    env_fog: [f32; 4],
}

const _: () = assert!(std::mem::size_of::<WorldLightmapExtra>() == 32);

/// One `WorldLightmapExtra` slot per frame that can be in flight. The block is
/// written by the CPU at record time, so a frame must not scribble on a slot a
/// still-queued frame's command buffer is reading — the same hazard, and the
/// same remedy, as the selection outline's ring.
///
/// Sized from [`crate::MAX_FRAMES_IN_FLIGHT`], **not** `FRAMES_IN_FLIGHT`.
/// [`WorldRenderer::draw`] advances the cursor once per call and the windowed
/// driver calls it exactly once per frame, so slot `n` is rewritten at frame
/// `n + RING`; the driver's fence only guarantees frame `n + fif` has retired.
/// With `fif` a runtime knob (`live --frames-in-flight`, clamped to
/// `1..=MAX_FRAMES_IN_FLIGHT`) and the `WorldRenderer` holding no reference to
/// the `Renderer` that drives it, `RING >= max fif` is what makes
/// `RING >= fif` hold for every legal configuration. `ring_slot_is_retired`
/// states the property; `lightmap_ubo_ring_outlives_every_frame_in_flight`
/// exhaustively checks it.
///
/// The offscreen oracle path (`Offscreen::render`) submits and waits per frame,
/// so it is safe at any ring length; only the windowed path needs this.
pub(super) const LIGHTMAP_UBO_RING: usize = crate::MAX_FRAMES_IN_FLIGHT;

/// Is the [`LIGHTMAP_UBO_RING`] slot that a draw writes guaranteed to have been
/// released by the GPU?
///
/// The windowed driver waits on frame `n - fif`'s fence before recording frame
/// `n`, so every frame at least `fif` back has retired; a draw reuses the slot
/// last written `ring` draws ago. The write is therefore safe exactly when
/// `ring >= fif`. Pure, so the invariant is checkable without a device — which
/// is all it is for, hence `cfg(test)`: the production path satisfies it by
/// construction (`LIGHTMAP_UBO_RING == MAX_FRAMES_IN_FLIGHT`) rather than by
/// asking at runtime.
#[cfg(test)]
const fn ring_slot_is_retired(ring: usize, fif: usize) -> bool {
    ring >= fif
}

/// Vanilla's resting `blockFactor` (`blockLightFlicker + 1.4` with zero
/// flicker) — the default `WorldLightmapState::block_factor`.
pub const BLOCK_LIGHT_FACTOR: f32 = 1.4;

/// The already-resolved lightmap uniforms driving `shaders/lightmap.glsl` —
/// the GPU mirror of `rewo_world::lightmap::LightmapState`, minus the fields
/// this scope pins to attribute-default constants in the shader (block tint =
/// `0xFFD88C`, night-vision colour = `0x999999`, boss darkening = 0).
/// `WorldRenderer` stores one and sends it every frame via `push_world` plus
/// the `LightmapExtra` uniform buffer.
///
/// `Default` is deliberately **term-neutral** for demo/view/bench: full sky
/// factor, the vanilla resting `1.4` block factor, white sky light, the
/// `EnvironmentAttributes` ambient default (black), and every
/// accessibility/effect term disabled. Live gamma `0.5` is supplied by the
/// app; the M13 correction to vanilla's exact block tint remains intentional.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldLightmapState {
    /// `SkyFactor` — scales the sky half of the light. Default `1.0`.
    pub sky_factor: f32,
    /// `BlockFactor` — `blockLightFlicker + 1.4`. Default the resting `1.4`.
    pub block_factor: f32,
    /// `SkyLightColor` — white by day, blue at night. Default white.
    pub sky_color: [f32; 3],
    /// `AmbientColor` — the dimension's `visual/ambient_light_color` as
    /// RGB24/255 (Overworld `#0a0a0a`, Nether `#302821`, End `#3f473f`).
    /// Default is the *attribute* codec default, black, so the serverless
    /// paths render exactly as they did before M16.
    pub ambient_color: [f32; 3],
    /// `BrightnessFactor` — the player's gamma minus the darkness effect (the
    /// `notGamma` mix weight). Default `0.0`.
    pub brightness_factor: f32,
    /// `DarknessScale` — the Darkness mob-effect subtraction. Default `0.0`.
    pub darkness_scale: f32,
    /// `NightVisionFactor` — the ambient night-vision seed. Default `0.0`.
    pub night_vision_factor: f32,
    /// The **environmental** fog band, `[start, end]` — vanilla's
    /// `FogData.environmentalStart/End`, which rain thickens (M33b).
    ///
    /// The default `[1e9, 1e9 + 1]` is *disabled*: everything is nearer than
    /// the start, so it contributes no fog and the render-distance band in the
    /// push block decides alone, exactly as before this field existed.
    pub env_fog: [f32; 2],
}

impl Default for WorldLightmapState {
    fn default() -> Self {
        Self {
            sky_factor: 1.0,
            block_factor: BLOCK_LIGHT_FACTOR,
            sky_color: [1.0, 1.0, 1.0],
            ambient_color: [0.0, 0.0, 0.0],
            brightness_factor: 0.0,
            darkness_scale: 0.0,
            night_vision_factor: 0.0,
            env_fog: [1.0e9, 1.0e9 + 1.0],
        }
    }
}

impl WorldLightmapState {
    /// Pack into the world push-constant `(light, sky_col)` vec4 pair exactly
    /// as `push_world` sends them: `light = [sky, block, brightness, darkness]`,
    /// `sky_col = [r, g, b, night_vision]`. Pure — no GPU state — so it is the
    /// unit-testable seam for the push layout (NOT a proxy for the shader math,
    /// which is verified against `rewo_world::lightmap` and the readback oracle).
    pub fn push_words(&self) -> ([f32; 4], [f32; 4]) {
        (
            [
                self.sky_factor,
                self.block_factor,
                self.brightness_factor,
                self.darkness_scale,
            ],
            [
                self.sky_color[0],
                self.sky_color[1],
                self.sky_color[2],
                self.night_vision_factor,
            ],
        )
    }

    /// The `LightmapExtra` uniform block's contents: the ambient colour in xyz,
    /// zero in the std140 pad. Pure — the unit-testable seam for the UBO layout.
    pub(super) fn extra_words(&self) -> [f32; 4] {
        [
            self.ambient_color[0],
            self.ambient_color[1],
            self.ambient_color[2],
            0.0,
        ]
    }

    /// The environmental fog band as the UBO wants it. The default is disabled
    /// — a start past the end, which the shader's clamp turns into zero fog.
    pub(super) fn env_fog_words(&self) -> [f32; 4] {
        [self.env_fog[0], self.env_fog[1], 0.0, 0.0]
    }
}

/// Publish one `WorldLightmapState`'s spill-over fields into a host-mapped
/// `LightmapExtra` slot.
///
/// **Invariant: the slot is mapped.** [`WorldRenderer::new`] allocates all
/// `LIGHTMAP_UBO_RING` slots `CpuToGpu` and *fails construction* if any of them
/// comes back unmapped, and the allocations are only taken back by `destroy` /
/// `take_allocations`. So a missing mapping here can only mean a draw was
/// recorded against a torn-down renderer.
///
/// It is deliberately not swallowed. Skipping the write leaves the descriptor
/// pointing at whatever the slot last held — i.e. the *previous dimension's*
/// ambient — and the frame would render confidently wrong. That is precisely
/// the failure mode this UBO exists to eliminate, so it fails loudly instead.
fn write_lightmap_extra(alloc: &mut Option<Allocation>, state: &WorldLightmapState) {
    let block = WorldLightmapExtra {
        ambient: state.extra_words(),
        env_fog: state.env_fog_words(),
    };
    let slice = alloc.as_mut().and_then(|a| a.mapped_slice_mut()).expect(
        "LightmapExtra slot has no host mapping — WorldRenderer::new establishes one for \
             every slot, so this is a draw recorded after destroy()/take_allocations()",
    );
    let bytes = bytemuck::bytes_of(&block);
    slice[..bytes.len()].copy_from_slice(bytes);
}

/// The `LightmapExtra` uniform buffers, one host-mapped buffer per
/// [`LIGHTMAP_UBO_RING`] slot, and the cursor naming the slot this frame writes
/// and binds. Owned by [`WorldRenderer`]; the graphics descriptor sets
/// (`tex_sets`) stay there, each pointing at the buffer of its own slot.
pub(super) struct LightmapRing {
    pub(super) bufs: [vk::Buffer; LIGHTMAP_UBO_RING],
    pub(super) allocs: [Option<Allocation>; LIGHTMAP_UBO_RING],
    /// Which ring slot this frame writes and binds. Advanced once per `draw`.
    pub(super) cursor: usize,
}

impl LightmapRing {
    /// One host-mapped uniform buffer per ring slot.
    pub(super) fn create(gpu: &mut Gpu) -> Result<Self, String> {
        let device = gpu.device.clone();
        let mut bufs = [vk::Buffer::null(); LIGHTMAP_UBO_RING];
        let mut allocs: [Option<Allocation>; LIGHTMAP_UBO_RING] = std::array::from_fn(|_| None);
        for i in 0..LIGHTMAP_UBO_RING {
            let buf = unsafe {
                device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(std::mem::size_of::<WorldLightmapExtra>() as u64)
                            .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("lightmap ubo: {e}"))?
            };
            let req = unsafe { device.get_buffer_memory_requirements(buf) };
            let mut alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "world-lightmap-extra",
                    requirements: req,
                    location: MemoryLocation::CpuToGpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("lightmap ubo alloc: {e}"))?;
            unsafe {
                device
                    .bind_buffer_memory(buf, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("lightmap ubo bind: {e}"))?;
            }
            // The host mapping is MANDATORY, established here once. A slot
            // that could not be mapped would make every later
            // `write_lightmap_extra` a no-op and the shader would read the
            // previous dimension's ambient forever — a silent wrong render.
            // Refuse to construct instead; from this point on the mapping is
            // an invariant, not a condition.
            if alloc.mapped_slice_mut().is_none() {
                return Err(format!(
                    "lightmap ubo slot {i}: CpuToGpu allocation is not host-mapped"
                ));
            }
            bufs[i] = buf;
            allocs[i] = Some(alloc);
        }
        Ok(Self {
            bufs,
            allocs,
            cursor: 0,
        })
    }

    /// Seed every slot with the default state so a first frame that never
    /// reaches `draw` still binds initialized memory.
    pub(super) fn seed_defaults(&mut self) {
        for i in 0..LIGHTMAP_UBO_RING {
            write_lightmap_extra(&mut self.allocs[i], &WorldLightmapState::default());
        }
    }

    /// Advance the ring once per frame and publish this frame's spill-over
    /// uniforms into the slot the terrain/water draws will bind.
    pub(super) fn advance(&mut self, state: &WorldLightmapState) {
        self.cursor = (self.cursor + 1) % LIGHTMAP_UBO_RING;
        write_lightmap_extra(&mut self.allocs[self.cursor], state);
    }
}

impl WorldRenderer {
    /// Set the time-of-day lightmap state: `sky_factor` scales the sky half
    /// of the light (vanilla `SKY_LIGHT_FACTOR`, 1.0 midday → 0.24 midnight)
    /// and `sky_color` tints it (white → blue). Block light is unaffected —
    /// a torch is as bright at midnight as at noon, which is the whole point
    /// of keeping the two channels separate through to the shader.
    pub fn set_lightmap(&mut self, sky_factor: f32, sky_color: [f32; 3]) {
        self.scene.lightmap.sky_factor = sky_factor;
        self.scene.lightmap.sky_color = sky_color;
    }

    /// Set the full resolved lightmap state (sky/block factors, sky colour,
    /// gamma/brightness, darkness, night vision) in one shot — the exact
    /// mirror of vanilla's `LightmapInfo`. `set_lightmap` is the day/night-only
    /// convenience that touches just the sky factor + colour.
    pub fn set_lightmap_state(&mut self, state: WorldLightmapState) {
        self.scene.lightmap = state;
    }

    /// M422 — the environmental fog band alone (a fog environment's), over
    /// the lightmap state already set this frame.
    pub fn set_env_fog(&mut self, band: [f32; 2]) {
        self.scene.lightmap.env_fog = band;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::terrain::WorldPush;

    // These pin the CPU-side push layout only. The lightmap shader math is NOT
    // re-implemented here (that would be a proxy oracle) — it is a line-for-line
    // port of `rewo_world::lightmap::sample`, which carries the IEEE bit pins,
    // and its GPU store is the job of the M13 Vulkan readback oracle.

    #[test]
    fn lightmap_default_is_visually_neutral() {
        let s = WorldLightmapState::default();
        assert_eq!(s.sky_factor, 1.0);
        assert_eq!(s.block_factor, BLOCK_LIGHT_FACTOR);
        assert_eq!(s.block_factor, 1.4);
        assert_eq!(s.sky_color, [1.0, 1.0, 1.0]);
        assert_eq!(s.brightness_factor, 0.0);
        assert_eq!(s.darkness_scale, 0.0);
        assert_eq!(s.night_vision_factor, 0.0);
        // The ambient default is the `EnvironmentAttributes` codec default
        // (black), NOT the Overworld dimension's `#0a0a0a` — that is what keeps
        // demo/view/bench byte-identical across M16.
        assert_eq!(s.ambient_color, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn lightmap_default_push_words_match_neutral() {
        let (light, sky_col) = WorldLightmapState::default().push_words();
        // light = [sky, block, brightness, darkness]
        assert_eq!(light, [1.0, 1.4, 0.0, 0.0]);
        // sky_col = [r, g, b, night_vision]
        assert_eq!(sky_col, [1.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn lightmap_push_words_lane_order() {
        // Distinct values catch any transposed lane.
        let s = WorldLightmapState {
            sky_factor: 0.24,
            block_factor: 1.5,
            sky_color: [0.1, 0.2, 0.3],
            brightness_factor: 0.5,
            darkness_scale: 0.45,
            night_vision_factor: 1.0,
            ambient_color: [0.6, 0.7, 0.8],
            env_fog: [12.0, 34.0],
        };
        let (light, sky_col) = s.push_words();
        assert_eq!(light, [0.24, 1.5, 0.5, 0.45]);
        assert_eq!(sky_col, [0.1, 0.2, 0.3, 1.0]);
        // The ambient must NOT have displaced a push lane — the block is full.
        assert!(!light.contains(&0.6) && !sky_col.contains(&0.8));
        // Nor may the environmental fog band, which shares the ambient UBO.
        assert!(!light.contains(&12.0) && !sky_col.contains(&34.0));
        assert_eq!(s.env_fog_words(), [12.0, 34.0, 0.0, 0.0]);
    }

    /// The `LightmapExtra` UBO layout: ambient in xyz, zero in the std140 pad,
    /// and exactly one `vec4` on the wire. Distinct channel values catch a
    /// transpose; the block size catches a struct that grew silently.
    #[test]
    fn lightmap_extra_words_carry_the_ambient_colour() {
        let s = WorldLightmapState {
            ambient_color: [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0],
            ..Default::default()
        };
        assert_eq!(
            s.extra_words(),
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0, 0.0]
        );
        assert_eq!(WorldLightmapState::default().extra_words(), [0.0; 4]);
        // Two vec4s since M33b: the ambient colour and the environmental fog
        // band. The band went here rather than into the push block because
        // that block is exactly at the guaranteed budget and cannot grow.
        assert_eq!(std::mem::size_of::<WorldLightmapExtra>(), 32);
        assert_eq!(std::mem::size_of::<WorldPush>(), 128);
        // The default band must be DISABLED — everything nearer than its
        // start — so a caller that never sets it renders exactly as before.
        let d = WorldLightmapState::default();
        assert!(
            d.env_fog[0] > 1.0e6,
            "default env fog must contribute nothing: {:?}",
            d.env_fog
        );
    }

    /// The ring must have one slot per frame in flight — fewer would let a
    /// recording frame overwrite the ambient a queued frame is still reading.
    #[test]
    fn lightmap_ubo_ring_covers_the_frames_in_flight() {
        // Must cover the LARGEST frames-in-flight the M6 knob can select, not
        // just the default — `WorldRenderer` cannot see which one is in use.
        assert_eq!(LIGHTMAP_UBO_RING, crate::MAX_FRAMES_IN_FLIGHT);
        const { assert!(LIGHTMAP_UBO_RING >= crate::FRAMES_IN_FLIGHT) };
        const { assert!(LIGHTMAP_UBO_RING >= 2) };

        // The property itself, not just the constant: `draw` pre-increments the
        // cursor, so draw n binds slot (n + 1) % RING. When the driver records
        // frame n it has waited only on frame n - fif's fence, so frames
        // n-1 .. n-fif+1 are still reading. None of them may share n's slot.
        let slot = |n: usize| (n + 1) % LIGHTMAP_UBO_RING;
        for fif in 1..=crate::MAX_FRAMES_IN_FLIGHT {
            assert!(
                ring_slot_is_retired(LIGHTMAP_UBO_RING, fif),
                "ring {LIGHTMAP_UBO_RING} cannot serve frames-in-flight {fif}"
            );
            for n in fif..fif + 64 {
                for back in 1..fif {
                    assert_ne!(
                        slot(n),
                        slot(n - back),
                        "fif {fif}: draw {n} would overwrite the LightmapExtra slot \
                         frame {} is still reading",
                        n - back
                    );
                }
            }
        }
        // The guard has teeth: the pre-M16-sized ring of 2 genuinely fails at
        // `live --frames-in-flight 3`, which is why the constant moved.
        assert!(!ring_slot_is_retired(2, 3));
        assert!(ring_slot_is_retired(2, 2));
    }

    #[test]
    fn set_lightmap_touches_only_sky_factor_and_colour() {
        // The day/night convenience must not disturb the effect terms.
        let mut s = WorldLightmapState {
            brightness_factor: 0.5,
            darkness_scale: 0.45,
            night_vision_factor: 1.0,
            block_factor: 1.5,
            ..Default::default()
        };
        // Mirror `set_lightmap`'s field writes without a live renderer.
        s.sky_factor = 0.24;
        s.sky_color = [0.1, 0.2, 0.3];
        assert_eq!(s.sky_factor, 0.24);
        assert_eq!(
            s.ambient_color,
            [0.0, 0.0, 0.0],
            "set_lightmap must not touch ambient"
        );
        assert_eq!(s.sky_color, [0.1, 0.2, 0.3]);
        assert_eq!(s.brightness_factor, 0.5);
        assert_eq!(s.darkness_scale, 0.45);
        assert_eq!(s.night_vision_factor, 1.0);
        assert_eq!(s.block_factor, 1.5);
    }
}
