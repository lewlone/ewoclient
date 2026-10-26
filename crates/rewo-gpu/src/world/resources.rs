//! Creating and tearing down a [`WorldRenderer`], and the buffer helpers the
//! sub-systems build their GPU memory with.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use super::arena::{self, Arena};
use super::cull::{self, Culler};
use super::lightmap::LightmapRing;
use super::lines;
use super::sky::{parse_fog_env, SkyGradient};
use super::terrain::{self, TerrainPass};
use super::textures::TextureArray;
use super::{
    GuiFrame, SceneFrame, SkyMode, WorldLightmapState, WorldRenderer, DEFAULT_FOG, DEFAULT_SKY,
};
use crate::Gpu;

impl WorldRenderer {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        tex_size: u32,
        layers: &[Vec<u8>],
    ) -> Result<Self, String> {
        let textures = TextureArray::create(gpu, tex_size, layers)?;
        // M197: the flow sprites' own array, one opaque-white placeholder layer
        // until `set_flow_textures` installs the baked ones.
        let flow_textures = TextureArray::create(gpu, super::textures::FLOW_TEX_SIZE, &[])?;
        let device = gpu.device.clone();

        let terrain_descriptors = terrain::create_terrain_descriptors(&device)?;

        let mut lm = LightmapRing::create(gpu)?;

        let terrain = TerrainPass::create(
            &device,
            color_format,
            terrain_descriptors,
            &textures,
            &flow_textures,
            &lm,
        )?;
        // Seed every slot with the default state so a first frame that
        // never reaches `draw` still binds initialized memory.
        lm.seed_defaults();

        let sky_gradient = SkyGradient::create(&device, color_format)?;

        // ---- selection outline (lines) + leash ribbon (M170) ----
        let (select_outline, leash_ribbon) = lines::create_line_overlays(gpu, color_format)?;

        // ---- compute cull descriptor + pipeline ----
        let cull_pipeline = cull::create_cull_pipeline(&device)?;

        // ---- mega-buffers + metadata + indirect + count ----
        let mega = arena::create_mega_buffers(gpu)?;
        let cull_bufs = cull::create_cull_buffers(gpu)?;

        // Cull descriptor set (meta, indirect cmds, count).
        let cull_descriptors = cull::create_cull_descriptors(&device, &cull_pipeline, &cull_bufs)?;

        // ---- async upload ring ----
        let upload_ring = arena::create_upload_ring(gpu)?;
        let (count_readback, count_readback_alloc) = create_host_buffer(
            gpu,
            16,
            vk::BufferUsageFlags::TRANSFER_DST,
            "count-readback",
        )?;

        Ok(Self {
            velvet_text: None,
            gui: GuiFrame::default(),
            scene: SceneFrame {
                camera_eye: [0.0; 3],
                lightmap: WorldLightmapState::default(),
                sky_tint: [1.0; 3],
                fog_tint: [1.0; 3],
                sky_base: DEFAULT_SKY,
                fog_base: DEFAULT_FOG,
                fog_final: None,
                fog: parse_fog_env(),
                sky_end: super::sky::DEFAULT_SKY_END,
                dark_disc: false,
                sky_mode: SkyMode::default(),
                celestial_state: crate::celestial::CelestialState::default(),
                end_flash: None,
                end_portal_time: 0.0,
                selection: None,
            },
            terrain,
            sky_gradient,
            select_outline,
            leash_ribbon,
            lm,
            textures,
            flow_textures,
            arena: Arena::new(mega, upload_ring),
            evicted_textures: Vec::new(),
            culler: Culler::new(
                cull_pipeline,
                cull_bufs,
                cull_descriptors,
                (count_readback, count_readback_alloc),
            ),
            color_format,
            gamma: None,
            outline: None,
            vignette: None,
            outline_drawn: std::cell::Cell::new(false),
            entities: None,
            celestial: None,
            end_sky: None,
            end_portal: None,
            clouds: None,
            border: None,
            weather: None,
            gui_items: None,
            gui_item_generation: 0,
            hand_generation: 0,
            hand_atlas_installs: 0,
            hand_region_updates: 0,
            container: None,
            screen: None,
            preview: None,
            hand: None,
            particles: None,
            crumbling: None,
            hud: None,
            locator: None,
            text: None,
            drawn_last_frame: 0,
            culled_last_frame: 0,
        })
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        gpu.wait_idle();
        if let Some(mut pass) = self.outline.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.vignette.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.celestial.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.end_sky.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.clouds.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.border.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.hand.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.preview.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.locator.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.container.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.screen.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.gui_items.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.particles.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.crumbling.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.weather.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.end_portal.take() {
            pass.destroy(gpu);
        }
        if let Some(mut pass) = self.entities.take() {
            pass.destroy(gpu);
        }
        if let Some(mut hud) = self.hud.take() {
            hud.destroy(gpu);
        }
        if let Some(mut vt) = self.velvet_text.take() {
            vt.destroy(gpu);
        }
        if let Some(mut text) = self.text.take() {
            text.destroy(gpu);
        }
        self.select_outline.destroy_ring(gpu);
        self.leash_ribbon.destroy_ring(gpu);
        unsafe {
            let device = &gpu.device;
            for s in &mut self.arena.upload_slots {
                device.destroy_fence(s.fence, None);
                device.destroy_buffer(s.staging, None);
            }
            device.destroy_command_pool(self.arena.upload_pool, None);
            self.culler.destroy_objects(device);
            self.terrain.destroy_pipelines(device);
            self.sky_gradient.destroy(device);
            self.select_outline.destroy_objects(device);
            self.leash_ribbon.destroy_objects(device);
            for b in self.lm.bufs {
                device.destroy_buffer(b, None);
            }
            self.terrain.destroy_layouts(device);
            self.textures.destroy_objects(device);
            self.flow_textures.destroy_objects(device);
            for b in [self.arena.vbuf, self.arena.ibuf, self.culler.readback] {
                device.destroy_buffer(b, None);
            }
        }
        let mut slot_allocs: Vec<Option<Allocation>> = self
            .arena
            .upload_slots
            .iter_mut()
            .map(|s| s.staging_alloc.take())
            .collect();
        for a in self.lm.allocs.iter_mut() {
            slot_allocs.push(a.take());
        }
        for a in [
            self.textures.image_alloc.take(),
            self.flow_textures.image_alloc.take(),
            self.arena.vbuf_alloc.take(),
            self.arena.ibuf_alloc.take(),
            self.culler.readback_alloc.take(),
        ]
        .into_iter()
        .chain(slot_allocs)
        .chain(self.culler.meta_allocs.iter_mut().map(|a| a.take()))
        .chain(self.culler.indirect_allocs.iter_mut().map(|a| a.take()))
        .chain(self.culler.count_allocs.iter_mut().map(|a| a.take()))
        .flatten()
        {
            let _ = gpu.allocator.free(a);
        }
    }
}

pub(super) fn create_device_buffer(
    gpu: &mut Gpu,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<(vk::Buffer, Allocation), String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = match gpu.allocator.allocate(&AllocationCreateDesc {
            name,
            requirements: req,
            location: MemoryLocation::GpuOnly,
            linear: true,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        }) {
            Ok(a) => a,
            // Not leaked: the arena's growth (M205) meets a refusal at run time.
            Err(e) => {
                gpu.device.destroy_buffer(buffer, None);
                return Err(format!("{name} alloc: {e}"));
            }
        };
        if let Err(e) = gpu
            .device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
        {
            gpu.device.destroy_buffer(buffer, None);
            let _ = gpu.allocator.free(alloc);
            return Err(format!("{name} bind: {e}"));
        }
        Ok((buffer, alloc))
    }
}

pub(super) fn create_host_buffer(
    gpu: &mut Gpu,
    size: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> Result<(vk::Buffer, Allocation), String> {
    unsafe {
        let buffer = gpu
            .device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(usage)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("{name}: {e}"))?;
        let req = gpu.device.get_buffer_memory_requirements(buffer);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name,
                requirements: req,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("{name} alloc: {e}"))?;
        gpu.device
            .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
            .map_err(|e| format!("{name} bind: {e}"))?;
        Ok((buffer, alloc))
    }
}
