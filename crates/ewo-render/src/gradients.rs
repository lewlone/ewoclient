//! Gradient shader constructors.
//!
//! skia-safe 0.93 deprecated the `gradient_shader` free functions in favour of
//! `gradient::shaders::*` over a `Gradient` (colours + interpolation) value.
//! Every gradient in the launcher and the in-game HUD is the same shape:
//! `Color4f` stops in the destination colour space, unpremultiplied
//! interpolation, no flags. These wrappers keep that one shape in one place,
//! so the call sites read like the old `gradient_shader::linear(..)` minus
//! the always-`None` colour space and flags.
//!
//! `Interpolation::default()` (no premul, destination space, shorter hue) is
//! exactly what the old API derived from empty flags, so the shaders are the
//! same objects the deprecated path builds.

use skia_safe::gradient::{shaders, Colors, Gradient, Interpolation};
use skia_safe::{scalar, Color4f, Matrix, Point, Shader, TileMode};

fn gradient<'a>(colors: &'a [Color4f], pos: Option<&'a [scalar]>, mode: TileMode) -> Gradient<'a> {
    Gradient::new(
        Colors::new(colors, pos, mode, None),
        Interpolation::default(),
    )
}

/// Linear gradient between two points.
pub fn linear<'a>(
    points: (impl Into<Point>, impl Into<Point>),
    colors: &[Color4f],
    pos: impl Into<Option<&'a [scalar]>>,
    mode: TileMode,
    local_matrix: impl Into<Option<&'a Matrix>>,
) -> Option<Shader> {
    shaders::linear_gradient(points, &gradient(colors, pos.into(), mode), local_matrix)
}

/// Radial gradient around `center`.
pub fn radial<'a>(
    center: impl Into<Point>,
    radius: scalar,
    colors: &[Color4f],
    pos: impl Into<Option<&'a [scalar]>>,
    mode: TileMode,
    local_matrix: impl Into<Option<&'a Matrix>>,
) -> Option<Shader> {
    shaders::radial_gradient(
        (center, radius),
        &gradient(colors, pos.into(), mode),
        local_matrix,
    )
}

/// Sweep gradient around `center`; `angles` defaults to the full 0..360.
pub fn sweep<'a>(
    center: impl Into<Point>,
    colors: &[Color4f],
    pos: impl Into<Option<&'a [scalar]>>,
    mode: TileMode,
    angles: impl Into<Option<(scalar, scalar)>>,
    local_matrix: impl Into<Option<&'a Matrix>>,
) -> Option<Shader> {
    shaders::sweep_gradient(
        center,
        angles.into().unwrap_or((0.0, 360.0)),
        &gradient(colors, pos.into(), mode),
        local_matrix,
    )
}
