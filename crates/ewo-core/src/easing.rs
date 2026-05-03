//! Easing curves. The project's signature is `silk` =
//! `cubic-bezier(0.22, 1, 0.36, 1)`, used for nearly every transition.

/// Cubic bezier easing as parameterized by CSS `cubic-bezier(x1, y1, x2, y2)`.
/// Control points are P0=(0,0), P1=(x1,y1), P2=(x2,y2), P3=(1,1). Evaluation
/// solves t given x via Newton-Raphson then evaluates y(t).
#[derive(Copy, Clone, Debug)]
pub struct CubicBezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicBezier {
    pub const SILK: Self = Self { x1: 0.22, y1: 1.0, x2: 0.36, y2: 1.0 };
    pub const LINEAR: Self = Self { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 };

    /// Evaluate the curve at progress `x` ∈ [0, 1]. Out-of-range inputs are
    /// clamped to the endpoints.
    pub fn eval(&self, x: f32) -> f32 {
        if !x.is_finite() || x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        // Newton-Raphson: refine t from initial guess t=x.
        let mut t = x;
        for _ in 0..8 {
            let dx = bezier(t, self.x1, self.x2) - x;
            let slope = bezier_slope(t, self.x1, self.x2);
            if slope.abs() < 1e-6 {
                break;
            }
            let step = dx / slope;
            t -= step;
            if step.abs() < 1e-6 {
                break;
            }
        }
        t = t.clamp(0.0, 1.0);
        bezier(t, self.y1, self.y2)
    }
}

#[inline]
fn bezier(t: f32, p1: f32, p2: f32) -> f32 {
    // CSS cubic-bezier with implicit P0=0, P3=1:
    //   B(t) = 3(1-t)²t·p1 + 3(1-t)t²·p2 + t³
    let u = 1.0 - t;
    3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
}

#[inline]
fn bezier_slope(t: f32, p1: f32, p2: f32) -> f32 {
    // dB/dt = 3(1-t)²·p1 + 6(1-t)t·(p2 - p1) + 3t²·(1 - p2)
    let u = 1.0 - t;
    3.0 * u * u * p1 + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
}
