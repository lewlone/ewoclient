//! The block texture array and its animated layers: creation, the blocking
//! initial upload with mip generation, and the per-tick re-upload of animated
//! frames (water / lava) through the arena's async upload ring — blended into
//! the next frame for the layers whose `.mcmeta` says `interpolate` (M195).

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use super::WorldRenderer;
use crate::Gpu;

/// One animated texture-array layer (mirror of rewo-data's
/// `AnimatedLayer` — this crate stays free of a rewo-data dependency).
pub struct LayerAnimation {
    pub layer: u16,
    /// 16×16 RGBA frames (pre-tinted).
    pub frames: Vec<Vec<u8>>,
    pub order: Vec<u32>,
    pub frametime: u32,
    /// Blend each frame into the next (M195), as vanilla's
    /// `ANIMATE_SPRITE_INTERPOLATE` pipeline does.
    pub interpolate: bool,
}

/// What an animated layer shows at game tick `tick`: `(frame, next,
/// progress)`, with `progress` in thousandths.
///
/// Rewo's clock is the session tick, so vanilla's per-sprite counters are
/// derived from it: the frame is `order[(tick / frametime) % len]` and
/// `subFrame` is `tick % frametime` (every interpolating block texture in
/// 26.2 has one uniform `frametime`). `SpriteContents.AnimationState.
/// drawToAtlas` then passes `(int)((float)subFrame / time * 1000.0F)` and the
/// frame after the current one, wrapping (`(frame + 1) % frames.size()`). A
/// layer that does not interpolate has `next == frame` and progress 0.
pub fn anim_sample(a: &LayerAnimation, tick: u64) -> Option<(usize, usize, u32)> {
    if a.order.is_empty() || a.frames.is_empty() {
        return None;
    }
    let time = u64::from(a.frametime.max(1));
    let step = (tick / time) as usize % a.order.len();
    let frame = a.order[step] as usize % a.frames.len();
    if !a.interpolate {
        return Some((frame, frame, 0));
    }
    let next = a.order[(step + 1) % a.order.len()] as usize % a.frames.len();
    let progress = (tick % time) as f32 / time as f32;
    Some((frame, next, (progress * 1000.0) as u32))
}

/// `animate_sprite_interpolate.fsh` on bytes: `mix(current, next,
/// progress / 1000.0)` per channel of each mip level, sampled from and
/// written to RGBA8 UNORM, so the blend is in the stored (gamma) bytes and
/// the store rounds to nearest.
pub fn blend_chain(current: &[u8], next: &[u8], progress_milli: u32) -> Vec<u8> {
    let p = progress_milli as f32 / 1000.0;
    current
        .iter()
        .zip(next)
        .map(|(&c, &n)| {
            let v = f32::from(c) / 255.0 * (1.0 - p) + f32::from(n) / 255.0 * p;
            (v * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

pub(super) fn generate_mips(tex_size: u32, mip_levels: u32, base: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![base.to_vec()];
    let mut prev_size = tex_size as usize;
    for _ in 1..mip_levels {
        let size = (prev_size / 2).max(1);
        let prev = out.last().unwrap();
        let mut mip = vec![0u8; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        let sy = (y * 2 + dy).min(prev_size - 1);
                        let sx = (x * 2 + dx).min(prev_size - 1);
                        sum += prev[(sy * prev_size + sx) * 4 + c] as u32;
                    }
                    mip[(y * size + x) * 4 + c] = (sum / 4) as u8;
                }
            }
        }
        out.push(mip);
        prev_size = size;
    }
    out
}

/// The CPU side of [`upload_texture_array`]: every layer's full mip chain laid
/// out mip-major (all layers' mip 0, then all layers' mip 1, …) in one staging
/// blob, with the byte offset each mip level starts at. An empty `layers` is
/// one opaque-white layer.
fn texture_staging(tex_size: u32, mip_levels: u32, layers: &[Vec<u8>]) -> (Vec<u8>, Vec<u64>) {
    let mut staging_data = Vec::new();
    let mut mip_offsets = Vec::new();
    let per_layer_mips: Vec<Vec<Vec<u8>>> = if layers.is_empty() {
        vec![generate_mips(
            tex_size,
            mip_levels,
            &vec![255u8; (tex_size * tex_size * 4) as usize],
        )]
    } else {
        layers
            .iter()
            .map(|l| generate_mips(tex_size, mip_levels, l))
            .collect()
    };
    for mip in 0..mip_levels as usize {
        mip_offsets.push(staging_data.len() as u64);
        for layer_mips in &per_layer_mips {
            staging_data.extend_from_slice(&layer_mips[mip]);
        }
    }
    (staging_data, mip_offsets)
}

/// `pub(crate)` so the M37 particle pass can build its own layered texture
/// through the same upload path rather than duplicating it.
pub(crate) fn upload_texture_array(
    gpu: &mut Gpu,
    image: vk::Image,
    tex_size: u32,
    mip_levels: u32,
    layers: &[Vec<u8>],
) -> Result<(), String> {
    let layer_count = layers.len().max(1);
    let (staging_data, mip_offsets) = texture_staging(tex_size, mip_levels, layers);

    unsafe {
        let device = gpu.device.clone();
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(staging_data.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("staging: {e}"))?;
        let req = device.get_buffer_memory_requirements(staging);
        let mut staging_alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "texture-staging",
                requirements: req,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, staging_alloc.memory(), staging_alloc.offset())
            .map_err(|e| format!("staging bind: {e}"))?;
        staging_alloc
            .mapped_slice_mut()
            .ok_or("staging not mapped")?[..staging_data.len()]
            .copy_from_slice(&staging_data);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("upload pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("upload cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("begin: {e}"))?;
        let full_range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(mip_levels)
            .base_array_layer(0)
            .layer_count(layer_count as u32);
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TOP_OF_PIPE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(full_range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );
        let mut regions = Vec::new();
        for mip in 0..mip_levels {
            let size = (tex_size >> mip).max(1);
            regions.push(
                vk::BufferImageCopy::default()
                    .buffer_offset(mip_offsets[mip as usize])
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(mip)
                            .base_array_layer(0)
                            .layer_count(layer_count as u32),
                    )
                    .image_extent(vk::Extent3D {
                        width: size,
                        height: size,
                        depth: 1,
                    }),
            );
        }
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &regions,
        );
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(full_range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device
            .end_command_buffer(cb)
            .map_err(|e| format!("end: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        device
            .queue_submit2(
                gpu.graphics_queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                fence,
            )
            .map_err(|e| format!("submit: {e}"))?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .map_err(|e| format!("wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(staging_alloc);
    }
    Ok(())
}

/// The block texture array (an sRGB 2D-array with a full mip chain), its
/// sampler, and the animated layers (water / lava) re-uploaded into it as
/// `anim_tick` advances.
pub(super) struct TextureArray {
    pub(super) sampler: vk::Sampler,
    image: vk::Image,
    pub(super) image_alloc: Option<Allocation>,
    pub(super) view: vk::ImageView,
    tex_size: u32,
    mip_levels: u32,
    animations: Vec<LayerAnimation>,
    /// Every animation frame's full mip chain, concatenated level by level —
    /// built once in `set_animations` so a frame change is a memcpy.
    anim_mips: Vec<Vec<Vec<u8>>>,
    /// What each animation last uploaded: [`anim_sample`]'s triple.
    anim_last: Vec<Option<(usize, usize, u32)>>,
}

impl TextureArray {
    /// Create the image, upload `layers` (blocking, with a full mip chain),
    /// and create its view and sampler.
    pub(super) fn create(gpu: &mut Gpu, tex_size: u32, layers: &[Vec<u8>]) -> Result<Self, String> {
        unsafe {
            let layer_count = layers.len().max(1) as u32;
            let mip_levels = 32 - tex_size.leading_zeros();

            // ---- texture array ----
            let image = gpu
                .device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_SRGB)
                        .extent(vk::Extent3D {
                            width: tex_size,
                            height: tex_size,
                            depth: 1,
                        })
                        .mip_levels(mip_levels)
                        .array_layers(layer_count)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        // TRANSFER_SRC for the gates' readback (M195).
                        .usage(
                            vk::ImageUsageFlags::SAMPLED
                                | vk::ImageUsageFlags::TRANSFER_DST
                                | vk::ImageUsageFlags::TRANSFER_SRC,
                        )
                        .initial_layout(vk::ImageLayout::UNDEFINED),
                    None,
                )
                .map_err(|e| format!("texture array: {e}"))?;
            let req = gpu.device.get_image_memory_requirements(image);
            let image_alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "block-texture-array",
                    requirements: req,
                    location: MemoryLocation::GpuOnly,
                    linear: false,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("texture alloc: {e}"))?;
            gpu.device
                .bind_image_memory(image, image_alloc.memory(), image_alloc.offset())
                .map_err(|e| format!("texture bind: {e}"))?;
            upload_texture_array(gpu, image, tex_size, mip_levels, layers)?;

            let device = gpu.device.clone();
            let view = device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                        .format(vk::Format::R8G8B8A8_SRGB)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .base_mip_level(0)
                                .level_count(mip_levels)
                                .base_array_layer(0)
                                .layer_count(layer_count),
                        ),
                    None,
                )
                .map_err(|e| format!("texture view: {e}"))?;
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT)
                        .max_lod(mip_levels as f32),
                    None,
                )
                .map_err(|e| format!("sampler: {e}"))?;
            Ok(Self {
                sampler,
                image,
                image_alloc: Some(image_alloc),
                view,
                tex_size,
                mip_levels,
                animations: Vec::new(),
                anim_mips: Vec::new(),
                anim_last: Vec::new(),
            })
        }
    }

    pub(super) fn destroy_objects(&self, device: &ash::Device) {
        unsafe {
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
    }
}

impl WorldRenderer {
    /// Register animated texture layers (from the bake). Frames are
    /// re-uploaded into the texture array as `anim_tick` advances.
    pub fn set_animations(&mut self, animations: Vec<LayerAnimation>) {
        self.textures.anim_last = vec![None; animations.len()];
        self.textures.anim_mips = animations
            .iter()
            .map(|a| {
                a.frames
                    .iter()
                    .map(|f| {
                        generate_mips(self.textures.tex_size, self.textures.mip_levels, f).concat()
                    })
                    .collect()
            })
            .collect();
        self.textures.animations = animations;
    }

    /// Advance texture animations to the given game tick (20 Hz), and
    /// re-upload any layer whose frame changed. A few KB per change —
    /// water/lava tick every `frametime` (2) ticks. An interpolating layer
    /// (M195) changes on every tick of its frame, as vanilla redraws it.
    pub fn anim_tick(&mut self, gpu: &mut Gpu, tick: u64) -> Result<(), String> {
        for i in 0..self.textures.animations.len() {
            let a = &self.textures.animations[i];
            let Some(sample) = anim_sample(a, tick) else {
                continue;
            };
            if self.textures.anim_last[i] == Some(sample) {
                continue;
            }
            self.textures.anim_last[i] = Some(sample);
            let layer = a.layer;
            let (frame, next, progress) = sample;
            let mips = &self.textures.anim_mips[i];
            let chain = if progress == 0 {
                mips[frame].clone()
            } else {
                blend_chain(&mips[frame], &mips[next], progress)
            };
            self.upload_layer_chain(gpu, layer, &chain)?;
        }
        Ok(())
    }

    /// Replace one texture-array layer's content (all mip levels) via an
    /// async slot upload (no CPU wait — same-queue ordering keeps the
    /// frame's sampling safe, and the in-cb barriers order the layer's
    /// shader reads around the copy).
    fn upload_layer_chain(
        &mut self,
        gpu: &mut Gpu,
        layer: u16,
        chain: &[u8],
    ) -> Result<(), String> {
        let total = chain.len() as u64;
        let (slot, base, cb) = self.arena.stage(gpu, total)?;
        unsafe {
            let alloc = self.arena.upload_slots[slot]
                .staging_alloc
                .as_mut()
                .unwrap();
            let slice = alloc.mapped_slice_mut().ok_or("staging not mapped")?;
            slice[base as usize..base as usize + chain.len()].copy_from_slice(chain);
            let mut off = base as usize;
            let mut copies = Vec::with_capacity(self.textures.mip_levels as usize);
            let mut size = self.textures.tex_size;
            for level in 0..self.textures.mip_levels as usize {
                let len = (size * size * 4) as usize;
                copies.push(
                    vk::BufferImageCopy::default()
                        .buffer_offset(off as u64)
                        .image_subresource(
                            vk::ImageSubresourceLayers::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .mip_level(level as u32)
                                .base_array_layer(layer as u32)
                                .layer_count(1),
                        )
                        .image_extent(vk::Extent3D {
                            width: size,
                            height: size,
                            depth: 1,
                        }),
                );
                off += len;
                size = (size / 2).max(1);
            }

            let staging = self.arena.upload_slots[slot].staging;
            let device = &gpu.device;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(self.textures.mip_levels)
                .base_array_layer(layer as u32)
                .layer_count(1);
            let to_dst = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(self.textures.image)
                .subresource_range(range);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
            );
            device.cmd_copy_buffer_to_image(
                cb,
                staging,
                self.textures.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &copies,
            );
            let to_read = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(self.textures.image)
                .subresource_range(range);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&to_read)),
            );
        }
        Ok(())
    }
}

#[cfg(feature = "gates")]
impl WorldRenderer {
    /// Read back one mip level of one texture-array layer as RGBA bytes —
    /// for the gates only (M195's `animshot` grades what `anim_tick` actually
    /// uploaded). Flushes the open upload batch first and waits for the
    /// device, so it is never called on a live frame.
    pub fn read_texture_layer(
        &mut self,
        gpu: &mut Gpu,
        layer: u16,
        level: u32,
    ) -> Result<Vec<u8>, String> {
        self.arena.flush_uploads(gpu)?;
        let size = (self.textures.tex_size >> level).max(1);
        let len = u64::from(size * size * 4);
        unsafe {
            let device = gpu.device.clone();
            device
                .device_wait_idle()
                .map_err(|e| format!("readback idle: {e}"))?;
            let buffer = device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(len)
                        .usage(vk::BufferUsageFlags::TRANSFER_DST)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(|e| format!("readback buffer: {e}"))?;
            let req = device.get_buffer_memory_requirements(buffer);
            let alloc = gpu
                .allocator
                .allocate(&AllocationCreateDesc {
                    name: "texture-readback",
                    requirements: req,
                    location: MemoryLocation::GpuToCpu,
                    linear: true,
                    allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                })
                .map_err(|e| format!("readback alloc: {e}"))?;
            device
                .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                .map_err(|e| format!("readback bind: {e}"))?;
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                    None,
                )
                .map_err(|e| format!("readback pool: {e}"))?;
            let cb = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(|e| format!("readback cb: {e}"))?[0];
            device
                .begin_command_buffer(
                    cb,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|e| format!("readback begin: {e}"))?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(level)
                .level_count(1)
                .base_array_layer(u32::from(layer))
                .layer_count(1);
            let to_src = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(self.textures.image)
                .subresource_range(range);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_src)),
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(level)
                        .base_array_layer(u32::from(layer))
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: size,
                    height: size,
                    depth: 1,
                });
            device.cmd_copy_image_to_buffer(
                cb,
                self.textures.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[region],
            );
            let back = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(self.textures.image)
                .subresource_range(range);
            let host = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .buffer(buffer)
                .size(len);
            device.cmd_pipeline_barrier2(
                cb,
                &vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&back))
                    .buffer_memory_barriers(std::slice::from_ref(&host)),
            );
            device
                .end_command_buffer(cb)
                .map_err(|e| format!("readback end: {e}"))?;
            let fence = device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .map_err(|e| format!("readback fence: {e}"))?;
            let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
            let submitted = device
                .queue_submit2(
                    gpu.graphics_queue,
                    &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                    fence,
                )
                .map_err(|e| format!("readback submit: {e}"))
                .and_then(|()| {
                    device
                        .wait_for_fences(&[fence], true, u64::MAX)
                        .map_err(|e| format!("readback wait: {e}"))
                });
            let bytes = submitted.and_then(|()| {
                alloc
                    .mapped_slice()
                    .map(|s| s[..len as usize].to_vec())
                    .ok_or_else(|| "readback not mapped".to_string())
            });
            device.destroy_fence(fence, None);
            device.destroy_command_pool(pool, None);
            device.destroy_buffer(buffer, None);
            let _ = gpu.allocator.free(alloc);
            bytes
        }
    }
}

#[cfg(test)]
mod m195_interpolate {
    use super::*;

    fn anim(order: Vec<u32>, frames: usize, frametime: u32, interpolate: bool) -> LayerAnimation {
        LayerAnimation {
            layer: 0,
            frames: (0..frames).map(|f| vec![f as u8; 4]).collect(),
            order,
            frametime,
            interpolate,
        }
    }

    /// `drawToAtlas`: the frame on screen, the one after it (wrapping) and
    /// `(int)((float)subFrame / time * 1000)` — magma's three frames at
    /// frametime 8.
    #[test]
    fn the_sample_follows_the_sprite_counters() {
        let magma = anim(vec![0, 1, 2], 3, 8, true);
        assert_eq!(anim_sample(&magma, 0), Some((0, 1, 0)));
        assert_eq!(anim_sample(&magma, 4), Some((0, 1, 500)));
        assert_eq!(anim_sample(&magma, 7), Some((0, 1, 875)));
        assert_eq!(anim_sample(&magma, 8), Some((1, 2, 0)));
        assert_eq!(
            anim_sample(&magma, 23),
            Some((2, 0, 875)),
            "wraps to frame 0"
        );
        // A third that does not divide evenly: 1/3 * 1000 truncates.
        let three = anim(vec![0, 1], 2, 3, true);
        assert_eq!(anim_sample(&three, 1), Some((0, 1, 333)));
        assert_eq!(anim_sample(&three, 2), Some((0, 1, 666)));
        // Without `interpolate` the next frame is the frame itself.
        let water = anim(vec![0, 1], 2, 2, false);
        assert_eq!(anim_sample(&water, 1), Some((0, 0, 0)));
        assert_eq!(anim_sample(&water, 2), Some((1, 1, 0)));
        assert_eq!(anim_sample(&anim(vec![], 2, 2, true), 5), None);
    }

    /// `mix(current, next, p)` on bytes, rounded to nearest on the store.
    #[test]
    fn the_blend_is_mix_on_the_stored_bytes() {
        assert_eq!(blend_chain(&[0, 255, 10], &[255, 0, 10], 0), [0, 255, 10]);
        assert_eq!(blend_chain(&[0, 255], &[255, 0], 1000), [255, 0]);
        // 200 * 0.25 = 50; 100 + 100 * 0.25 = 125; 100 + 100 * 0.875 = 187.5.
        assert_eq!(blend_chain(&[0, 100], &[200, 200], 250), [50, 125]);
        assert_eq!(blend_chain(&[100], &[200], 875), [188]);
    }
}
