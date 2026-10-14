//! Offscreen layers that are cached and blitted 1:1.
//!
//! The launcher paints in logical pixels: `events.rs` puts a
//! `canvas.scale(scale_factor)` on the frame and lays everything out against
//! the logical viewport. A cache created at the *logical* size and drawn under
//! that scale is upscaled by the scale factor on every blit, which softens the
//! backdrop and the inner glow on any HiDPI display (125 % on 1440p, 200 %
//! on 4K).
//!
//! So a cached layer is created at the **physical** size the canvas will show
//! it at, drawn into with the same scale applied (its content is as sharp as
//! drawing straight to the screen), and blitted with the transform reset so
//! one layer pixel is exactly one device pixel.

use skia_safe::{AlphaType, Canvas, ColorType, Image, ImageInfo, Surface};

/// Pixel size of a layer covering a logical `w` x `h` area.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LayerSize {
    pub px_w: i32,
    pub px_h: i32,
}

/// Per-axis scale from the canvas's local units to device pixels.
pub fn device_scale(canvas: &Canvas) -> (f32, f32) {
    let m = canvas.local_to_device_as_3x3();
    (m.scale_x().hypot(m.skew_y()), m.skew_x().hypot(m.scale_y()))
}

/// `dy` (canvas-local units) rounded to a whole number of *device* pixels.
///
/// Text-bearing layers may fade and translate but must land on whole device
/// pixels, or glyphs are resampled and soften for the duration of the move
/// (non-negotiable #2). Rounding in logical units is not enough under a HiDPI
/// scale (at 125 % a whole logical pixel is 1.25 device pixels), so round in
/// physical pixels and convert back.
pub fn snap_dy(canvas: &Canvas, dy: f32) -> f32 {
    let (_, sy) = device_scale(canvas);
    if sy > 0.0 && sy.is_finite() {
        (dy * sy).round() / sy
    } else {
        dy.round()
    }
}

/// The physical size of a layer covering logical `w` x `h` under `canvas`'s
/// current transform, or `None` for a degenerate area.
pub fn layer_size(canvas: &Canvas, w: f32, h: f32) -> Option<LayerSize> {
    let (sx, sy) = device_scale(canvas);
    let px_w = (w * sx).round() as i32;
    let px_h = (h * sy).round() as i32;
    (px_w > 0 && px_h > 0).then_some(LayerSize { px_w, px_h })
}

/// A new GPU-compatible surface of `size`, with the canvas's scale applied so
/// the caller draws in the same logical coordinates it would draw on `canvas`.
/// `None` when no offscreen can be allocated (e.g. a non-GPU canvas).
pub fn new_layer(canvas: &Canvas, size: LayerSize) -> Option<Surface> {
    let info = ImageInfo::new(
        (size.px_w, size.px_h),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    let mut surface = canvas.new_surface(&info, None)?;
    let (sx, sy) = device_scale(canvas);
    if sx != 1.0 || sy != 1.0 {
        surface.canvas().scale((sx, sy));
    }
    Some(surface)
}

/// Draw `image` (made by [`new_layer`]) with its top-left on the device pixel
/// that logical `(0, 0)` lands on, with no resampling. The current clip still
/// applies.
pub fn blit_layer(canvas: &Canvas, image: &Image) {
    let origin = canvas.local_to_device_as_3x3().map_point((0.0, 0.0));
    let saved = canvas.save();
    canvas.reset_matrix();
    canvas.draw_image(image, (origin.x.round(), origin.y.round()), None);
    canvas.restore_to_count(saved);
}

#[cfg(test)]
mod tests {
    use super::*;
    use skia_safe::surfaces;

    #[test]
    fn snap_dy_lands_on_whole_device_pixels() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0, 3.0] {
            let mut s = surfaces::raster_n32_premul((64, 64)).expect("surface");
            s.canvas().scale((scale, scale));
            for i in 0..=140 {
                let dy = i as f32 * 0.1;
                let snapped = snap_dy(s.canvas(), dy);
                let px = snapped * scale;
                assert!(
                    (px - px.round()).abs() < 1e-3,
                    "scale {scale}: {dy} -> {snapped} is {px} device px"
                );
                assert!((snapped - dy).abs() <= 0.5 / scale + 1e-4);
            }
        }
    }
}
