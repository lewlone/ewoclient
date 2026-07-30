//! The locator bar — `LocatorBar` + `ContextualBar` (M83).
//!
//! A 182×5 strip above the hotbar carrying one 9×9 dot per tracked waypoint,
//! placed by the **bearing** from the camera to the waypoint, plus a small
//! up/down arrow when the subject is off the top or bottom of the screen. The
//! wire half is [`rewo_net::waypoints`].
//!
//! # What identifies a waypoint, and how a bearing becomes a screen position
//!
//! The identifier never reaches this module: a dot's *placement* is a pure
//! function of the camera and the waypoint's body, and its *colour* falls back
//! to `hashCode` of the identifier only when the icon carries none. What does
//! reach here is [`LocatorWaypoint::is_camera_entity`] — vanilla's one use of
//! the identifier in the render loop is `id.left().equals(cameraEntity.
//! getUUID())`, and that comparison is REWO_PLAN §0.0 gotcha 13 in both
//! directions (see the field's doc).
//!
//! The bearing chain, verbatim from `extractRenderState`:
//!
//! ```text
//! angle       = yawAngleToCamera(...)                 // degreesDifference, (-180, 180]
//! visible    ⟺ !(angle <= -60) && !(angle > 60)
//! screenMid   = Mth.ceil((guiWidth - 9) / 2.0f)
//! dotPosition = Mth.floor(angle * 173.0 / 2.0 / 60.0)
//! dot         at (screenMid + dotPosition, top - 2), 9×9, tinted
//! arrow       at (screenMid + dotPosition + 1, top ± 6), 7×5, untinted
//! ```
//!
//! # Six things that read backwards
//!
//! 1. **The visibility window is half-open, and the open end is the LEFT one.**
//!    `!(angle <= -60) && !(angle > 60)` is `(-60, 60]`, so a waypoint exactly
//!    60° clockwise draws and one exactly 60° anticlockwise does not. Written
//!    as `angle.abs() <= 60.0` it is `[-60, 60]`; as `< 60.0`, `(-60, 60)`.
//!    Neither is vanilla, and both differ only on a measure-zero set — which
//!    is precisely why nobody would notice.
//!
//! 2. **An `EMPTY` waypoint is visible, at dead centre.** `yawAngleToCamera`
//!    returns `Double.NaN` for it, and *every* comparison against NaN is
//!    false, so both halves of the guard pass. `Mth.floor(NaN)` is
//!    `(int)Math.floor(NaN)` = `(int)NaN` = **0**. Rust agrees by a different
//!    route (a saturating float→int cast sends NaN to 0), so the transcription
//!    is literal. A guard written with `abs()` rejects NaN and hides it.
//!
//! 3. **The dot travels 173 px, not 182.** `angle * 173.0 / 2.0 / 60.0` spans
//!    ±86.5 px over ±60°, and 173 is `182 - 9` — the bar's width less the
//!    dot's, so a dot at the extreme sits flush inside the strip. Using the
//!    bar width would push half the dot out at each end.
//!
//! 4. **Two different points measure the two different quantities.** The
//!    bearing uses `camera.position()` (the render camera, i.e. the eye); the
//!    distance that picks the sprite uses `waypoint.distanceSquared(
//!    cameraEntity)`, which is `Entity.distanceToSqr` against the entity's
//!    **feet**. Using one for both is a ~1.6-block error in the sprite
//!    selection and invisible in the placement.
//!
//! 5. **`isBehindCamera` no longer means what it says.** `pitchDirectionToCamera`
//!    tests `pointOnScreen.z > 1.0`, which was "behind the camera" back when
//!    the projection ran near→far. 26.2's `Projection.getMatrix` swaps them
//!    (`float near = this.zFar; float far = this.zNear;`) — reversed-Z — so a
//!    point behind the camera now projects to a *negative* z and the branch is
//!    reachable only within the real near plane. Transcribed as written; see
//!    [`project_point_to_screen`].
//!
//! 6. **Having waypoints is not enough to show the bar.** `nextContextualInfoState`
//!    puts the XP bar in front of the locator bar for 100 ticks after an XP
//!    change (`willPrioritizeExperienceInfo`), so on a server that both grants
//!    XP and transmits waypoints the strip disappears and comes back. See
//!    [`contextual_bar`].
//!
//! From the 26.2 decompile: `client/gui/contextualbar/{LocatorBar,ContextualBar}.java`,
//! `client/gui/Hud.java`, `client/resources/WaypointStyle.java`,
//! `world/waypoints/TrackedWaypoint.java`, `util/{Mth,ARGB}.java`.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::hud::HudSpriteData;
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

// ── `ContextualBar` + `LocatorBar` constants ────────────────────────────────

/// `ContextualBar.WIDTH`.
pub const BAR_W: i32 = 182;
/// `ContextualBar.HEIGHT`.
pub const BAR_H: i32 = 5;
/// `ContextualBar.MARGIN_BOTTOM`.
pub const MARGIN_BOTTOM: i32 = 24;
/// `LocatorBar.DOT_SIZE`.
pub const DOT_SIZE: i32 = 9;
/// `LocatorBar.VISIBLE_DEGREE_RANGE`.
pub const VISIBLE_DEGREE_RANGE: f64 = 60.0;
/// `LocatorBar.ARROW_WIDTH`.
pub const ARROW_W: i32 = 7;
/// `LocatorBar.ARROW_HEIGHT`.
pub const ARROW_H: i32 = 5;
/// `LocatorBar.ARROW_LEFT` — the arrow is one pixel right of its dot, because
/// it is 7 wide against the dot's 9 and `(9 - 7) / 2 == 1`.
pub const ARROW_LEFT: i32 = 1;
/// `LocatorBar.ARROW_PADDING` — the gap the ±6 vertical offsets are built from
/// (`5 + 1` above, `5 + 1` below).
pub const ARROW_PADDING: i32 = 1;

/// The dot's travel, in pixels, over the full ±60°: `182 - 9`. Named because
/// the literal `173.0` in `extractRenderState` is the one number in the whole
/// file that is not obviously derived from a constant beside it.
pub const DOT_TRAVEL: f64 = (BAR_W - DOT_SIZE) as f64;

/// `WaypointStyle.DEFAULT_NEAR_DISTANCE`.
pub const DEFAULT_NEAR_DISTANCE: i32 = 128;
/// `WaypointStyle.DEFAULT_FAR_DISTANCE`.
pub const DEFAULT_FAR_DISTANCE: i32 = 332;

// ── `Mth` and `ARGB`, transcribed ───────────────────────────────────────────

/// `Mth.FRAC_BIAS` — `Double.longBitsToDouble(4805340802404319232L)`.
const FRAC_BIAS: f64 = f64::from_bits(4_805_340_802_404_319_232u64);
const LUT_SIZE: usize = 257;

struct AtanTables {
    asin: [f64; LUT_SIZE],
    cos: [f64; LUT_SIZE],
}

fn atan_tables() -> &'static AtanTables {
    use std::sync::OnceLock;
    static T: OnceLock<AtanTables> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = AtanTables {
            asin: [0.0; LUT_SIZE],
            cos: [0.0; LUT_SIZE],
        };
        // `Mth`'s static block, verbatim.
        for ind in 0..LUT_SIZE {
            let v = ind as f64 / 256.0;
            let asinv = v.asin();
            t.cos[ind] = asinv.cos();
            t.asin[ind] = asinv;
        }
        t
    })
}

/// `Mth.fastInvSqrt` — the Quake reciprocal-square-root seed plus one Newton
/// step, on doubles. Deprecated in vanilla and still called from `Mth.atan2`.
fn fast_inv_sqrt(x: f64) -> f64 {
    let xhalf = 0.5 * x;
    let i = x.to_bits() as i64;
    let i = 6_910_469_410_427_058_090i64 - (i >> 1);
    let x = f64::from_bits(i as u64);
    x * (1.5 - xhalf * x * x)
}

/// `Mth.atan2` — vanilla's table-driven approximation, **not** `Math.atan2`.
///
/// Transcribed rather than substituted because it is what produces the angle
/// the dot's pixel column is floored from; the two agree to about 1e-6 rad,
/// which is 1e-4 px at the bar's 1.44 px/degree, so a substitution is *almost*
/// invisible — and "almost invisible" is the class of thing this project keeps
/// getting wrong. The gate measures the actual disagreement rather than
/// asserting there is none.
pub fn mth_atan2(mut y: f64, mut x: f64) -> f64 {
    let d2 = x * x + y * y;
    if d2.is_nan() {
        return f64::NAN;
    }
    let neg_y = y < 0.0;
    if neg_y {
        y = -y;
    }
    let neg_x = x < 0.0;
    if neg_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    let rinv = fast_inv_sqrt(d2);
    x *= rinv;
    y *= rinv;
    let yp = FRAC_BIAS + y;
    // `(int)Double.doubleToRawLongBits(yp)` — Java narrows a long to an int by
    // keeping the low 32 bits. The bias is chosen so those bits are the table
    // index for `y ∈ [0, 1]`.
    let index = (yp.to_bits() as u32) as usize;
    let t = atan_tables();
    let phi = t.asin[index.min(LUT_SIZE - 1)];
    let c_phi = t.cos[index.min(LUT_SIZE - 1)];
    let s_phi = yp - FRAC_BIAS;
    let sd = y * c_phi - x * s_phi;
    let d = (6.0 + sd * sd) * sd * 0.166_666_666_666_666_66;
    let mut theta = phi + d;
    if steep {
        theta = std::f64::consts::FRAC_PI_2 - theta;
    }
    if neg_x {
        theta = std::f64::consts::PI - theta;
    }
    if neg_y {
        theta = -theta;
    }
    theta
}

/// `Mth.wrapDegrees(float)` — normalises into `(-180, 180]`.
pub fn wrap_degrees(angle: f32) -> f32 {
    let mut a = angle % 360.0;
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// `Mth.degreesDifference(from, to)` = `wrapDegrees(to - from)`.
pub fn degrees_difference(from: f32, to: f32) -> f32 {
    wrap_degrees(to - from)
}

/// `java.util.UUID.hashCode()`:
/// `long hilo = msb ^ lsb; return ((int)(hilo >> 32)) ^ (int)hilo;`
pub fn java_uuid_hash(uuid: u128) -> i32 {
    let msb = (uuid >> 64) as u64;
    let lsb = uuid as u64;
    let hilo = msb ^ lsb;
    ((hilo >> 32) as u32 as i32) ^ (hilo as u32 as i32)
}

/// `java.lang.String.hashCode()` — `s[0]*31^(n-1) + …`, over **UTF-16 code
/// units**, with Java's wrapping `int` arithmetic.
pub fn java_string_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for u in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u as i32);
    }
    h
}

/// `ARGB.setBrightness(color, brightness)` — an RGB→HSV round trip that keeps
/// hue and saturation and *replaces* value.
///
/// The locator bar calls it with `0.9`, so a dot derived from a hash is always
/// a little short of full brightness however the hash's bytes fall.
pub fn argb_set_brightness(color: u32, brightness: f32) -> u32 {
    let mut red = ((color >> 16) & 0xFF) as i32;
    let mut green = ((color >> 8) & 0xFF) as i32;
    let mut blue = (color & 0xFF) as i32;
    let alpha = (color >> 24) as i32;
    let rgb_max = red.max(green).max(blue);
    let rgb_min = red.min(green).min(blue);
    let rgb_constant_range = (rgb_max - rgb_min) as f32;
    let saturation = if rgb_max != 0 {
        rgb_constant_range / rgb_max as f32
    } else {
        0.0
    };
    let hue = if saturation == 0.0 {
        0.0f32
    } else {
        let constant_red = (rgb_max - red) as f32 / rgb_constant_range;
        let constant_green = (rgb_max - green) as f32 / rgb_constant_range;
        let constant_blue = (rgb_max - blue) as f32 / rgb_constant_range;
        let mut hue = if red == rgb_max {
            constant_blue - constant_green
        } else if green == rgb_max {
            2.0 + constant_red - constant_blue
        } else {
            4.0 + constant_green - constant_red
        };
        hue /= 6.0;
        if hue < 0.0 {
            hue += 1.0;
        }
        hue
    };

    if saturation == 0.0 {
        let v = java_round(brightness * 255.0);
        return argb(alpha, v, v, v);
    }

    let color_wheel_segment = (hue - hue.floor()) * 6.0;
    let color_wheel_offset = color_wheel_segment - color_wheel_segment.floor();
    let primary = brightness * (1.0 - saturation);
    let secondary = brightness * (1.0 - saturation * color_wheel_offset);
    let tertiary = brightness * (1.0 - saturation * (1.0 - color_wheel_offset));
    match color_wheel_segment as i32 {
        0 => {
            red = java_round(brightness * 255.0);
            green = java_round(tertiary * 255.0);
            blue = java_round(primary * 255.0);
        }
        1 => {
            red = java_round(secondary * 255.0);
            green = java_round(brightness * 255.0);
            blue = java_round(primary * 255.0);
        }
        2 => {
            red = java_round(primary * 255.0);
            green = java_round(brightness * 255.0);
            blue = java_round(tertiary * 255.0);
        }
        3 => {
            red = java_round(primary * 255.0);
            green = java_round(secondary * 255.0);
            blue = java_round(brightness * 255.0);
        }
        4 => {
            red = java_round(tertiary * 255.0);
            green = java_round(primary * 255.0);
            blue = java_round(brightness * 255.0);
        }
        _ => {
            red = java_round(brightness * 255.0);
            green = java_round(primary * 255.0);
            blue = java_round(secondary * 255.0);
        }
    }
    argb(alpha, red, green, blue)
}

/// `Math.round(float)` — `floor(x + 0.5)`, which is **not** Rust's
/// `f32::round` (that is half-away-from-zero). They differ at every negative
/// half-integer; the inputs here are non-negative, so this is a faithfulness
/// note rather than a live bug, and it costs one line to be right anyway.
fn java_round(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}

/// `ARGB.color(a, r, g, b)`.
fn argb(a: i32, r: i32, g: i32, b: i32) -> u32 {
    (((a & 0xFF) as u32) << 24)
        | (((r & 0xFF) as u32) << 16)
        | (((g & 0xFF) as u32) << 8)
        | ((b & 0xFF) as u32)
}

// ── `WaypointStyle` ─────────────────────────────────────────────────────────

/// One `waypoint_style/*.json`, resolved to atlas dot indices.
#[derive(Clone, Debug)]
pub struct WaypointStyle {
    /// The style's registry key, e.g. `minecraft:default`.
    pub key: String,
    pub near_distance: i32,
    pub far_distance: i32,
    /// Indices into the atlas's dot list, in the JSON's order.
    pub sprites: Vec<u16>,
}

impl WaypointStyle {
    /// `WaypointStyle.sprite(float distance)`.
    ///
    /// The two special cases in the middle are not an optimisation of the
    /// `lerpInt` below them: for a 3-sprite list `lerpInt` would return index
    /// 1 or 2 depending on where in the band the distance falls, and the
    /// explicit branch pins it at 1. Dropping them changes which sprite a
    /// three-entry style shows.
    pub fn sprite(&self, distance: f32) -> u16 {
        if self.sprites.is_empty() {
            return 0;
        }
        if distance < self.near_distance as f32 {
            return self.sprites[0];
        }
        if distance >= self.far_distance as f32 {
            return *self.sprites.last().unwrap();
        }
        if self.sprites.len() == 1 {
            return self.sprites[0];
        }
        if self.sprites.len() == 3 {
            return self.sprites[1];
        }
        // `Mth.lerpInt(alpha, p0, p1)` = `p0 + floor(alpha * (p1 - p0))`.
        let alpha = (distance - self.near_distance as f32)
            / (self.far_distance - self.near_distance) as f32;
        let hi = self.sprites.len() as i32 - 1;
        let index = 1 + (alpha * (hi - 1) as f32).floor() as i32;
        self.sprites[index.clamp(0, hi) as usize]
    }
}

// ── Inputs ──────────────────────────────────────────────────────────────────

/// The waypoint body, as the renderer needs it.
///
/// Mirrors `rewo_net::waypoints::WaypointContents` rather than importing it,
/// for the reason `HudGauges` exists: `rewo-gpu` takes no dependency on
/// `rewo-net`, and the `Vec3i` case additionally carries something the wire
/// does not (see `entity_eye`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WaypointSubject {
    /// `EmptyWaypoint` — no position at all. Its bearing is `NaN` and its
    /// distance `+∞`; both are load-bearing (see the module doc).
    Empty,
    Vec3i {
        x: i32,
        y: i32,
        z: i32,
        /// `Vec3iWaypoint.position` prefers the **tracked entity's**
        /// interpolated eye position over the transmitted block, but only when
        /// `e.blockPosition().distManhattan(vector) <= 3` — a staleness guard,
        /// since the position packet and the entity's own movement packets
        /// arrive independently. `None` when the client has no such entity,
        /// which is the normal case for anything outside render distance.
        ///
        /// Resolved by the caller because the lookup is `level.getEntity(uuid)`
        /// and the identifier does not otherwise reach this module.
        entity_eye: Option<[f64; 3]>,
    },
    Chunk {
        x: i32,
        z: i32,
    },
    /// The wire's f32, still in **radians**.
    Azimuth {
        radians: f32,
    },
}

/// One tracked waypoint, resolved for drawing.
#[derive(Clone, Debug)]
pub struct LocatorWaypoint {
    pub subject: WaypointSubject,
    /// The dot tint, opaque ARGB — either `icon.color` or the identifier hash
    /// through `setBrightness(…, 0.9)`. Resolved by the caller because the
    /// fallback needs the identifier.
    pub color: u32,
    /// Index into the style table handed to [`markers`].
    pub style: usize,
    /// `waypoint.id().left().map(u -> u.equals(cameraEntity.getUUID()))`.
    ///
    /// **REWO_PLAN §0.0 gotcha 13, both ends.** The subject of a waypoint may
    /// be an entity `EntityTable` holds; the *observer* never is, because the
    /// server sends no `add_entity` for you. So this flag cannot be derived
    /// from the table — it has to come from the session's own UUID. A
    /// vanilla server never sends you your own waypoint
    /// (`ServerWaypointManager.createConnection` opens `if (player !=
    /// waypoint)`), so a client that dropped this check looks correct on a
    /// vanilla server and paints a permanent dot at the player's own bearing
    /// the moment a plugin or a datapack transmits one.
    pub is_camera_entity: bool,
}

/// The camera, as `extractRenderState` reads it.
#[derive(Clone, Copy, Debug)]
pub struct LocatorCamera {
    /// `camera.yaw()`, degrees.
    pub yaw: f32,
    /// `mainCamera.xRot()`, degrees, positive **downward** (Minecraft's sign).
    pub pitch: f32,
    /// `mainCamera.getFov()` — the **vertical** field of view, degrees.
    pub fov: f32,
    /// `camera.position()` — the render camera. Every bearing is measured from
    /// here.
    pub camera_pos: [f64; 3],
    /// `cameraEntity.position()` — the entity's **feet**. Only
    /// `distanceSquared` uses this, and only to pick a sprite.
    pub entity_pos: [f64; 3],
    /// The projection's real near and far planes, in that order. 26.2 hands
    /// them to `setPerspective` the other way round; see
    /// [`project_point_to_screen`].
    pub near: f32,
    pub far: f32,
}

// ── Outputs ─────────────────────────────────────────────────────────────────

/// `TrackedWaypoint.PitchDirection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PitchDirection {
    None,
    Up,
    Down,
}

/// One dot (and its optional arrow), in **GUI pixels**.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocatorMarker {
    /// The bearing that placed it, kept so a gate can assert the mapping.
    pub angle: f64,
    pub x: i32,
    pub y: i32,
    /// The atlas dot index the style picked for this distance.
    pub sprite: u16,
    pub color: u32,
    pub pitch: PitchDirection,
    /// `(x, y)` of the 7×5 arrow, when `pitch != None`.
    pub arrow: Option<(i32, i32)>,
}

// ── Geometry ────────────────────────────────────────────────────────────────

/// `ContextualBar.left` — `(guiScaledWidth - 182) / 2`, an **integer** divide,
/// so an odd GUI width biases the bar one pixel left.
pub fn bar_left(gui_w: i32) -> i32 {
    (gui_w - BAR_W) / 2
}

/// `ContextualBar.top` — `guiScaledHeight - 24 - 5`.
pub fn bar_top(gui_h: i32) -> i32 {
    gui_h - MARGIN_BOTTOM - BAR_H
}

/// `Mth.ceil((graphics.guiWidth() - 9) / 2.0F)`.
///
/// A **float** divide then a ceiling, against `bar_left`'s integer divide —
/// so the dot column and the strip are centred by two different roundings and
/// disagree by a pixel at odd widths. That is vanilla's, and the arithmetic is
/// transcribed rather than unified.
pub fn screen_middle(gui_w: i32) -> i32 {
    (((gui_w - DOT_SIZE) as f32) / 2.0).ceil() as i32
}

/// `!(angle <= -60.0) && !(angle > 60.0)` — the half-open window, NaN-passing.
pub fn is_visible(angle: f64) -> bool {
    !(angle <= -VISIBLE_DEGREE_RANGE) && !(angle > VISIBLE_DEGREE_RANGE)
}

/// `Mth.floor(angle * 173.0 / 2.0 / 60.0)`.
///
/// The literal order matters for nothing here (all three are exact powers of a
/// small integer over a float) but is kept anyway. NaN floors to 0 in both
/// languages.
pub fn dot_offset(angle: f64) -> i32 {
    (angle * DOT_TRAVEL / 2.0 / VISIBLE_DEGREE_RANGE).floor() as i32
}

/// `Vec3.atCenterOf(BlockPos)`.
fn at_center_of(x: i32, y: i32, z: i32) -> [f64; 3] {
    [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5]
}

/// `ChunkPos.getMiddleBlockPosition(y)` — `(x*16 + 8, y, z*16 + 8)`.
fn chunk_middle(cx: i32, cz: i32, y: i32) -> [f64; 3] {
    at_center_of(cx * 16 + 8, y, cz * 16 + 8)
}

/// `Vec3iWaypoint.position` / `ChunkWaypoint.position`, i.e. the point a
/// bearing is taken to. `None` for `EMPTY`, which has no position at all.
pub fn subject_position(subject: &WaypointSubject, cam: &LocatorCamera) -> Option<[f64; 3]> {
    match *subject {
        WaypointSubject::Empty => None,
        // The Manhattan-3 staleness test lives in the caller, which is the
        // only place the entity is in scope; `entity_eye` is already the
        // decision's outcome.
        WaypointSubject::Vec3i {
            x,
            y,
            z,
            entity_eye,
        } => Some(entity_eye.unwrap_or_else(|| at_center_of(x, y, z))),
        // `this.position(cameraPosition.y())` — the chunk's middle column at
        // the **camera's** height, cast to int. A chunk waypoint therefore has
        // no vertical offset from you by construction, which is why its
        // `pitchDirectionToCamera` consults only the horizon.
        WaypointSubject::Chunk { x, z } => Some(chunk_middle(x, z, cam.camera_pos[1] as i32)),
        WaypointSubject::Azimuth { .. } => None,
    }
}

/// `TrackedWaypoint.yawAngleToCamera`.
///
/// The shared shape is `direction = cameraPos - waypointPos`, **rotated
/// clockwise 90°** (`Vec3(-z, y, x)`), then `Mth.atan2(dir.z, dir.x)` in
/// degrees, then `degreesDifference` against the camera yaw. The rotation is
/// what converts a world-axis bearing into Minecraft's yaw convention (0° =
/// +Z, growing clockwise); dropping it rotates every dot by a quarter turn.
pub fn yaw_angle_to_camera(subject: &WaypointSubject, cam: &LocatorCamera) -> f64 {
    match *subject {
        WaypointSubject::Empty => f64::NAN,
        WaypointSubject::Azimuth { radians } => {
            degrees_difference(cam.yaw, radians * (180.0 / std::f32::consts::PI)) as f64
        }
        _ => {
            let Some(p) = subject_position(subject, cam) else {
                return f64::NAN;
            };
            let d = [
                cam.camera_pos[0] - p[0],
                cam.camera_pos[1] - p[1],
                cam.camera_pos[2] - p[2],
            ];
            // `Vec3.rotateClockwise90` = `new Vec3(-z, y, x)`.
            let (rx, rz) = (-d[2], d[0]);
            let waypoint_angle = (mth_atan2(rz, rx) as f32) * (180.0 / std::f32::consts::PI);
            degrees_difference(cam.yaw, waypoint_angle) as f64
        }
    }
}

/// `TrackedWaypoint.distanceSquared(Entity)`.
///
/// `+∞` for `EMPTY` and `AZIMUTH` — neither has a position, and the infinity
/// is not a sentinel that gets filtered out: it flows into
/// `Mth.sqrt((float)…)` and then `WaypointStyle.sprite`, where `distance >=
/// farDistance` selects the **last** sprite. A far-away player therefore shows
/// the smallest dot, which is the whole visual grammar of the bar.
pub fn distance_squared(subject: &WaypointSubject, cam: &LocatorCamera) -> f64 {
    let p = match *subject {
        WaypointSubject::Empty | WaypointSubject::Azimuth { .. } => return f64::INFINITY,
        // Note this is the **raw block** centre, not `entity_eye`: vanilla's
        // `distanceSquared` reads `this.vector` directly and never consults
        // the entity, even though `yawAngleToCamera` does.
        WaypointSubject::Vec3i { x, y, z, .. } => at_center_of(x, y, z),
        // `fromEntity.getBlockY()`, not the camera's y.
        WaypointSubject::Chunk { x, z } => {
            chunk_middle(x, z, cam.entity_pos[1].floor() as i32)
        }
    };
    let d = [
        cam.entity_pos[0] - p[0],
        cam.entity_pos[1] - p[1],
        cam.entity_pos[2] - p[2],
    ];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// `GameRenderer.projectHorizonToScreen` —
/// `tan(xRot) / tan(fov / 2)`, with `±Infinity` outside `±90°`.
///
/// The sign convention is Minecraft's: `xRot` is positive looking **down**, so
/// looking down puts the horizon *above* the screen and this returns a
/// positive number, which the callers read as `UP`.
pub fn project_horizon_to_screen(pitch_deg: f32, fov_deg: f32) -> f64 {
    if pitch_deg <= -90.0 {
        return f64::NEG_INFINITY;
    }
    if pitch_deg >= 90.0 {
        return f64::INFINITY;
    }
    (pitch_deg as f64 * (std::f64::consts::PI / 180.0)).tan()
        / ((fov_deg / 2.0) as f64 * (std::f64::consts::PI / 180.0)).tan()
}

/// The `(y, z)` of `GameRenderer.projectPointToScreen(point)`, which is
/// `getViewRotationProjectionMatrix().transformProject(point - cameraPos)`.
///
/// Only two components are needed, and both are closed forms once the matrix
/// is known — so this reproduces the arithmetic rather than a matrix:
///
/// * `y_clip = v.y / tan(fov/2)`, `w_clip = -v.z` (JOML `setPerspective`'s
///   `m11` and `m23 = -1`), so `y_ndc = y_clip / w_clip`. **Aspect divides x
///   only**, so y needs no viewport at all.
/// * `z_clip = m22·v.z + m32` where, because `Projection.getMatrix` passes
///   `near = this.zFar; far = this.zNear`, JOML's `m22 = zFar/(zNear - zFar)`
///   and `m32 = zFar·zNear/(zNear - zFar)` evaluate to `n/(f - n)` and
///   `n·f/(f - n)` in the **real** near/far. So `z_ndc = -m22 - m32/v.z`,
///   which is `[0, 1]` in front, exceeds 1 only inside the real near plane,
///   and goes **negative** behind the camera. `pitchDirectionToCamera`'s
///   `pointOnScreen.z > 1.0` is named `isBehindCamera`; reversed-Z made that
///   name false, and the branch is transcribed as written rather than as
///   named.
pub fn project_point_to_screen(point: [f64; 3], cam: &LocatorCamera) -> (f64, f64) {
    let off = [
        point[0] - cam.camera_pos[0],
        point[1] - cam.camera_pos[1],
        point[2] - cam.camera_pos[2],
    ];
    // Minecraft's camera basis. `yaw` 0 looks **+Z** and grows toward −X;
    // `pitch` is positive **downward**. Only `up` and `forward` are needed —
    // the aspect ratio divides x alone, so no viewport enters this.
    let (sy, cy) = (cam.yaw as f64).to_radians().sin_cos();
    let (sp, cp) = (cam.pitch as f64).to_radians().sin_cos();
    let forward = [-sy * cp, -sp, cy * cp];
    // `cross(right, forward)` with `right = cross(forward, +Y)`.
    let up = [-sy * sp, cp, cy * sp];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // The camera looks down −Z in view space.
    let vz = -dot(off, forward);

    let h = ((cam.fov / 2.0) as f64 * (std::f64::consts::PI / 180.0)).tan();
    let w_clip = -vz;
    let y_clip = dot(off, up) / h;
    let (n, f) = (cam.near as f64, cam.far as f64);
    let m22 = n / (f - n);
    let m32 = n * f / (f - n);
    let z_clip = m22 * vz + m32;
    (y_clip / w_clip, z_clip / w_clip)
}

/// `TrackedWaypoint.pitchDirectionToCamera`.
pub fn pitch_direction(subject: &WaypointSubject, cam: &LocatorCamera) -> PitchDirection {
    match *subject {
        // `EmptyWaypoint` short-circuits to NONE without consulting anything.
        WaypointSubject::Empty => PitchDirection::None,
        // `ChunkWaypoint` and `AzimuthWaypoint` share the horizon-only rule.
        WaypointSubject::Chunk { .. } | WaypointSubject::Azimuth { .. } => {
            let horizon = project_horizon_to_screen(cam.pitch, cam.fov);
            if horizon < -1.0 {
                PitchDirection::Down
            } else if horizon > 1.0 {
                PitchDirection::Up
            } else {
                PitchDirection::None
            }
        }
        WaypointSubject::Vec3i { .. } => {
            let Some(p) = subject_position(subject, cam) else {
                return PitchDirection::None;
            };
            let (y, z) = project_point_to_screen(p, cam);
            let is_behind_camera = z > 1.0;
            let y_in_front = if is_behind_camera { -y } else { y };
            if y_in_front < -1.0 {
                return PitchDirection::Down;
            }
            if y_in_front > 1.0 {
                return PitchDirection::Up;
            }
            if is_behind_camera {
                if y > 0.0 {
                    return PitchDirection::Up;
                }
                if y < 0.0 {
                    return PitchDirection::Down;
                }
            }
            PitchDirection::None
        }
    }
}

/// `LocatorBar.extractRenderState`'s whole loop, as a draw list.
///
/// Draw order is `ClientWaypointManager.forEachWaypoint`'s: sorted by
/// `distanceSquared` **reversed**, so the farthest dot is emitted first and
/// the nearest lands on top. `sort_by` is stable and the input order is
/// [`rewo_net::waypoints::WaypointStore::iter_sorted`]'s deterministic one, so
/// ties (every `AZIMUTH` waypoint shares `+∞`) break by identifier rather than
/// by vanilla's unspecified `ConcurrentHashMap` order.
pub fn markers(
    waypoints: &[LocatorWaypoint],
    styles: &[WaypointStyle],
    cam: &LocatorCamera,
    gui_w: i32,
    gui_h: i32,
) -> Vec<LocatorMarker> {
    let top = bar_top(gui_h);
    let mid = screen_middle(gui_w);

    let mut ordered: Vec<(f64, &LocatorWaypoint)> = waypoints
        .iter()
        .map(|w| (distance_squared(&w.subject, cam), w))
        .collect();
    // `Comparator.comparingDouble(…).reversed()`. `total_cmp` orders NaN too;
    // `distanceSquared` never produces one, and a partial comparator that
    // panicked on the +∞ ties would be a Rust artefact.
    ordered.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut out = Vec::new();
    for (dist_sq, w) in ordered {
        if w.is_camera_entity {
            continue;
        }
        let angle = yaw_angle_to_camera(&w.subject, cam);
        if !is_visible(angle) {
            continue;
        }
        // `Mth.sqrt((float) distanceSquared)` — the f64 is narrowed **first**,
        // so an f64 sqrt would be a different number for large distances.
        let distance = (dist_sq as f32).sqrt();
        let sprite = styles
            .get(w.style)
            .map(|s| s.sprite(distance))
            .unwrap_or(0);
        let dot_x = mid + dot_offset(angle);
        let dot_y = top - 2;
        let pitch = pitch_direction(&w.subject, cam);
        let arrow = match pitch {
            PitchDirection::None => None,
            // `arrowTop = 6` for DOWN and `-6` for UP: the arrow sits *below*
            // the bar to point down and *above* it to point up, both by
            // `HEIGHT + ARROW_PADDING`.
            PitchDirection::Down => Some((dot_x + ARROW_LEFT, top + BAR_H + ARROW_PADDING)),
            PitchDirection::Up => Some((dot_x + ARROW_LEFT, top - BAR_H - ARROW_PADDING)),
        };
        out.push(LocatorMarker {
            angle,
            x: dot_x,
            y: dot_y,
            sprite,
            color: w.color,
            pitch,
            arrow,
        });
    }
    out
}

/// `Hud.nextContextualInfoState`, restricted to the two bars Rewo has.
///
/// Returns whether the **locator** bar is the contextual bar this frame.
/// Vanilla's third branch, `JUMPABLE_VEHICLE`, outranks both and is
/// unreachable here: Rewo models no rideable-jumping vehicle, so
/// `jumpableVehicle()` is permanently null and the two arms that consult it
/// collapse. Recorded rather than transcribed as dead code.
///
/// The half that is easy to get backwards: having waypoints does **not** win.
/// The XP bar takes the slot for 100 ticks after every XP change, so on a
/// server that does both, the strip blinks out on each orb pickup.
pub fn contextual_bar(has_waypoints: bool, has_experience: bool, xp_prioritised: bool) -> bool {
    has_waypoints && !(has_experience && xp_prioritised)
}

/// The two-frame arrow animation from
/// `locator_bar_arrow_{up,down}.png.mcmeta`: index 0 for 10 ticks, index 1 for
/// 4. The sprite file is **7×10**, i.e. two 7×5 frames stacked — reading it as
/// one 7×10 sprite and blitting the declared 7×5 shows the top frame forever
/// and never blinks.
pub fn arrow_frame(tick: i64) -> usize {
    const CYCLE: i64 = 14;
    if tick.rem_euclid(CYCLE) < 10 {
        0
    } else {
        1
    }
}

/// Expand the 12×5 nine-slice background to the 182×5 the bar blits.
///
/// `locator_bar_background.png` is **12×5**, not 182×5: its `.mcmeta` declares
/// `nine_slice` with `border {left: 5, right: 5, top: 1, bottom: 1}` and no
/// `stretch_inner`, and `blitSprite(…, 182, 5)` therefore takes
/// `blitNineSlicedSprite`'s middle branch — `height == nineSlice.height()`, so
/// a *horizontal* three-slice: the left 5 columns, the middle 2 **tiled**
/// (`stretchInner` defaults to false) across 172, then the right 5. Stretching
/// the middle instead would smear a 2-px pattern over 172 px.
pub fn expand_nine_slice(src: &HudSpriteData<'_>, out_w: u32) -> Vec<u8> {
    let (sw, sh) = (src.w, src.h);
    let mut out = vec![0u8; (out_w * sh * 4) as usize];
    let border_l = 5u32.min(out_w / 2);
    let border_r = 5u32.min(out_w / 2);
    let copy_col = |out: &mut [u8], dst_x: u32, src_x: u32| {
        for row in 0..sh {
            let s = ((row * sw + src_x) * 4) as usize;
            let d = ((row * out_w + dst_x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src.rgba[s..s + 4]);
        }
    };
    for i in 0..border_l.min(sw) {
        copy_col(&mut out, i, i);
    }
    for i in 0..border_r.min(sw) {
        copy_col(&mut out, out_w - border_r + i, sw - border_r + i);
    }
    let tile_w = sw - border_l - border_r;
    if tile_w > 0 {
        for i in 0..(out_w - border_l - border_r) {
            copy_col(&mut out, border_l + i, border_l + i % tile_w);
        }
    }
    out
}

// ── The pass ────────────────────────────────────────────────────────────────

const VERTEX_STRIDE: u64 = 32; // vec2 pos + vec2 uv + vec4 color
const MAX_VERTS: usize = 2048;
const RING: usize = 2;
const ATLAS_W: u32 = 256;
const ATLAS_H: u32 = 32;
/// How many dot sprites the atlas has room for on its one 9-px row.
pub const MAX_DOTS: usize = 24;

/// The jar's locator sprites plus the parsed style table.
pub struct LocatorSpritesData<'a> {
    /// 12×5, nine-slice.
    pub background: HudSpriteData<'a>,
    /// 7×10 — two 7×5 animation frames.
    pub arrow_up: HudSpriteData<'a>,
    pub arrow_down: HudSpriteData<'a>,
    /// 9×9 each, from `gui/sprites/hud/locator_bar_dot/`.
    pub dots: Vec<HudSpriteData<'a>>,
    /// One per `waypoint_style/*.json`, sprite indices already resolved
    /// against `dots`.
    pub styles: Vec<WaypointStyle>,
}

#[derive(Clone, Copy, Default)]
struct Rect {
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    w: f32,
    h: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

/// What the bar draws this frame.
#[derive(Clone, Debug, Default)]
pub struct LocatorBarState {
    pub markers: Vec<LocatorMarker>,
    /// Drives the arrow's two-frame animation.
    pub tick: i64,
}

pub struct LocatorBarPass {
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
    background: Rect,
    /// `[up frame 0, up frame 1, down frame 0, down frame 1]`.
    arrows: [Rect; 4],
    dots: Vec<Rect>,
    /// The synthesised stand-in for `MissingTextureAtlasSprite`.
    missing: Rect,
}

impl LocatorBarPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        sprites: &LocatorSpritesData<'_>,
    ) -> Result<Self, String> {
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let place = |dst: &mut [u8], rgba: &[u8], sw: u32, sh: u32, x: u32, y: u32| -> Rect {
            for row in 0..sh {
                let s = (row * sw * 4) as usize;
                let d = (((y + row) * ATLAS_W + x) * 4) as usize;
                dst[d..d + (sw * 4) as usize].copy_from_slice(&rgba[s..s + (sw * 4) as usize]);
            }
            Rect {
                u0: x as f32 / ATLAS_W as f32,
                v0: y as f32 / ATLAS_H as f32,
                u1: (x + sw) as f32 / ATLAS_W as f32,
                v1: (y + sh) as f32 / ATLAS_H as f32,
                w: sw as f32,
                h: sh as f32,
            }
        };

        // Row 0: the expanded 182×5 background.
        let bg = expand_nine_slice(&sprites.background, BAR_W as u32);
        let background = place(&mut atlas, &bg, BAR_W as u32, BAR_H as u32, 0, 0);

        // Row 1 (y = 8): the four 7×5 arrow frames. Each source is 7×10 with
        // frame 1 directly under frame 0, so a frame is a row-offset slice.
        let frame = |s: &HudSpriteData<'_>, i: u32| -> Vec<u8> {
            let start = (i * ARROW_H as u32 * s.w * 4) as usize;
            let len = (ARROW_H as u32 * s.w * 4) as usize;
            s.rgba[start..start + len].to_vec()
        };
        let mut arrows = [Rect::default(); 4];
        for (i, (src, base)) in [(&sprites.arrow_up, 0u32), (&sprites.arrow_down, 16)]
            .into_iter()
            .enumerate()
        {
            for f in 0..2u32 {
                let px = frame(src, f);
                arrows[i * 2 + f as usize] = place(
                    &mut atlas,
                    &px,
                    ARROW_W as u32,
                    ARROW_H as u32,
                    base + f * 8,
                    8,
                );
            }
        }

        // Row 2 (y = 16): the 9×9 dots, then the synthesised missing patch.
        let mut dots = Vec::new();
        for (i, d) in sprites.dots.iter().take(MAX_DOTS).enumerate() {
            dots.push(place(&mut atlas, d.rgba, d.w, d.h, i as u32 * 10, 16));
        }
        // `WaypointStyleManager.MISSING` names `MissingTextureAtlasSprite`,
        // which Rewo has no GUI atlas to resolve — so the patch is synthesised
        // as the usual magenta/black check. The *behaviour* around it is
        // vanilla's: `MISSING` is `WaypointStyle(0, 1, [it])`, whose near/far
        // put every real distance past `farDistance` and select its one entry.
        let mx = (MAX_DOTS as u32).min(sprites.dots.len() as u32 + 1) * 10;
        let mut miss = vec![0u8; (DOT_SIZE * DOT_SIZE * 4) as usize];
        for y in 0..DOT_SIZE as u32 {
            for x in 0..DOT_SIZE as u32 {
                let d = ((y * DOT_SIZE as u32 + x) * 4) as usize;
                let magenta = ((x / 4) + (y / 4)) % 2 == 0;
                miss[d] = if magenta { 0xF8 } else { 0 };
                miss[d + 1] = 0;
                miss[d + 2] = if magenta { 0xF8 } else { 0 };
                miss[d + 3] = 0xFF;
            }
        }
        let missing = place(
            &mut atlas,
            &miss,
            DOT_SIZE as u32,
            DOT_SIZE as u32,
            mx.min(ATLAS_W - DOT_SIZE as u32),
            16,
        );

        // UNORM, not SRGB — M50's rule. The dot is `texture * vertexColor` and
        // vanilla evaluates that product in **gamma** space (its GUI textures
        // carry no sRGB view). Sampling through an sRGB view would linearise
        // the texel first and a multiply in linear is a different quantity;
        // the shader re-encodes for the attachment instead.
        let (image, image_alloc, view) =
            crate::entities::create_glint_texture(gpu, &atlas, ATLAS_W, ATLAS_H)?;
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
                .map_err(|e| format!("locator sampler: {e}"))?;
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
                .map_err(|e| format!("locator set layout: {e}"))?;
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
                .map_err(|e| format!("locator pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("locator set: {e}"))?[0];
            let info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
            let ranges = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(8)];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&ranges),
                    None,
                )
                .map_err(|e| format!("locator layout: {e}"))?;
            let pipeline = build_pipeline(&device, layout, color_format)?;

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = Default::default();
            for i in 0..RING {
                let buf = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(VERTEX_STRIDE * MAX_VERTS as u64)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("locator buffer: {e}"))?;
                let req = device.get_buffer_memory_requirements(buf);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "locator_bar",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("locator alloc: {e}"))?;
                device
                    .bind_buffer_memory(buf, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("locator bind: {e}"))?;
                bufs[i] = buf;
                allocs[i] = Some(alloc);
            }

            Ok(LocatorBarPass {
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
                background,
                arrows,
                dots,
                missing,
            })
        }
    }

    /// The atlas index for a style's sprite, falling back to the synthesised
    /// missing patch. `u16::MAX` is the caller's "no style resolved".
    fn dot_rect(&self, sprite: u16) -> &Rect {
        self.dots.get(sprite as usize).unwrap_or(&self.missing)
    }

    pub fn draw(
        &mut self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        extent: vk::Extent2D,
        state: &LocatorBarState,
    ) {
        let (w, h) = (extent.width.max(1) as f32, extent.height.max(1) as f32);
        let scale = crate::hud::gui_scale(w, h);
        let (sw, sh) = (w / scale, h / scale);

        self.cursor = (self.cursor + 1) % RING;
        let mut v: Vec<Vertex> = Vec::with_capacity(128);
        let mut quad = |x: f32, y: f32, r: &Rect, color: u32| {
            let (px, py) = (x * scale, y * scale);
            let (pw, ph) = (r.w * scale, r.h * scale);
            // Gamma-space channels: the shader multiplies the raw texel by
            // these and re-encodes, so they are byte/255, not linearised.
            let c = [
                ((color >> 16) & 0xFF) as f32 / 255.0,
                ((color >> 8) & 0xFF) as f32 / 255.0,
                (color & 0xFF) as f32 / 255.0,
                ((color >> 24) & 0xFF) as f32 / 255.0,
            ];
            let corners = [
                ([px, py], [r.u0, r.v0]),
                ([px + pw, py], [r.u1, r.v0]),
                ([px + pw, py + ph], [r.u1, r.v1]),
                ([px, py], [r.u0, r.v0]),
                ([px + pw, py + ph], [r.u1, r.v1]),
                ([px, py + ph], [r.u0, r.v1]),
            ];
            for (pos, uv) in corners {
                if v.len() < MAX_VERTS {
                    v.push(Vertex {
                        pos,
                        uv,
                        color: c,
                    });
                }
            }
        };

        // `extractBackground` — untinted, at the contextual bar's own origin.
        quad(
            bar_left(sw as i32) as f32,
            bar_top(sh as i32) as f32,
            &self.background,
            0xFFFF_FFFF,
        );
        let frame = arrow_frame(state.tick);
        for m in &state.markers {
            quad(m.x as f32, m.y as f32, self.dot_rect(m.sprite), m.color);
            if let Some((ax, ay)) = m.arrow {
                // The 6-argument `blitSprite` overload, i.e. `color = -1`:
                // the arrow is **not** tinted with the dot's colour.
                let idx = match m.pitch {
                    PitchDirection::Up => frame,
                    _ => 2 + frame,
                };
                quad(ax as f32, ay as f32, &self.arrows[idx], 0xFFFF_FFFF);
            }
        }

        self.verts = v.len() as u32;
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    v.as_ptr() as *const u8,
                    v.len() * VERTEX_STRIDE as usize,
                )
            };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
        if self.verts == 0 {
            return;
        }

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

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/locator.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/locator.frag.spv")),
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
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
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
            .map_err(|(_, e)| format!("locator pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> LocatorCamera {
        LocatorCamera {
            yaw: 0.0,
            pitch: 0.0,
            fov: 70.0,
            // On a block centre, so a waypoint's own `atCenterOf` offset
            // cancels and a cardinal bearing is exactly 0 or ±90.
            camera_pos: [0.5, 64.0, 0.5],
            entity_pos: [0.5, 62.4, 0.5],
            near: 0.05,
            far: 1024.0,
        }
    }

    #[test]
    fn the_visible_window_is_half_open() {
        assert!(is_visible(60.0));
        assert!(!is_visible(60.000_001));
        assert!(!is_visible(-60.0));
        assert!(is_visible(-59.999_999));
        // NaN passes both halves — `!(NaN <= x)` and `!(NaN > x)` are true.
        assert!(is_visible(f64::NAN));
    }

    #[test]
    fn an_empty_waypoint_lands_at_dead_centre() {
        let c = cam();
        let angle = yaw_angle_to_camera(&WaypointSubject::Empty, &c);
        assert!(angle.is_nan());
        assert_eq!(dot_offset(angle), 0);
        assert_eq!(distance_squared(&WaypointSubject::Empty, &c), f64::INFINITY);
        assert_eq!(
            pitch_direction(&WaypointSubject::Empty, &c),
            PitchDirection::None
        );
    }

    #[test]
    fn the_dot_travels_173_pixels_not_182() {
        assert_eq!(dot_offset(60.0), 86);
        assert_eq!(dot_offset(-60.0), -87); // floor, not truncate
        assert_eq!(dot_offset(0.0), 0);
        assert_eq!(DOT_TRAVEL, 173.0);
    }

    #[test]
    fn the_two_centres_disagree_at_an_odd_width() {
        // 182-wide strip vs a 9-wide dot, one integer divide and one ceil.
        assert_eq!(bar_left(320), 69);
        assert_eq!(screen_middle(320), 156); // (320-9)/2 = 155.5 -> 156
        assert_eq!(bar_left(321), 69); // (321-182)/2 = 69 (truncated)
        assert_eq!(screen_middle(321), 156); // (321-9)/2 = 156.0
        assert_eq!(bar_top(240), 211);
    }

    #[test]
    fn a_waypoint_due_north_is_a_zero_bearing() {
        let c = cam();
        // Minecraft yaw 0 faces +Z. A waypoint at +Z should read ~0.
        let s = WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 100,
            entity_eye: None,
        };
        let a = yaw_angle_to_camera(&s, &c);
        assert!(a.abs() < 0.01, "bearing due +Z was {a}");
        // …and one due +X reads **−90**, not +90. Minecraft's yaw grows from
        // +Z toward −X, so with the camera facing +Z the +X axis is on your
        // left and the dot sits at the left end of the strip — which is also
        // exactly out of the half-open window.
        let s = WaypointSubject::Vec3i {
            x: 100,
            y: 64,
            z: 0,
            entity_eye: None,
        };
        let a = yaw_angle_to_camera(&s, &c);
        assert!((a + 90.0).abs() < 0.01, "bearing due +X was {a}");
        assert!(!is_visible(a));
    }

    #[test]
    fn the_azimuth_body_is_radians() {
        let c = cam();
        let s = WaypointSubject::Azimuth {
            radians: std::f32::consts::FRAC_PI_4,
        };
        // 45°, not 0.785°.
        let a = yaw_angle_to_camera(&s, &c);
        assert!((a - 45.0).abs() < 1e-4, "azimuth read {a}");
    }

    #[test]
    fn mth_atan2_tracks_the_platform_within_a_micro_radian() {
        let mut worst = 0.0f64;
        for i in -720..=720 {
            let t = i as f64 * 0.25 * std::f64::consts::PI / 180.0;
            let (y, x) = (t.sin() * 37.0, t.cos() * 37.0);
            let d = (mth_atan2(y, x) - y.atan2(x)).abs();
            if d > worst {
                worst = d;
            }
        }
        assert!(worst < 1e-5, "Mth.atan2 drifted by {worst} rad");
    }

    #[test]
    fn the_hash_fallback_is_javas() {
        // Two anchors computable by hand: UUID(0,0) hashes to 0, and the empty
        // string to 0. `setBrightness` of an all-zero colour takes the
        // saturation-zero branch and returns a grey at the requested value.
        assert_eq!(java_uuid_hash(0), 0);
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("a"), 97);
        assert_eq!(java_string_hash("ab"), 97 * 31 + 98);
        let grey = argb_set_brightness(0xFF00_0000, 0.9);
        let v = java_round(0.9 * 255.0);
        assert_eq!(grey, argb(255, v, v, v));
        // A saturated hue keeps its hue and takes the requested value — and
        // the value is **230**, not 229. `0.9f * 255.0f` rounds *up* to
        // exactly 229.5f in f32, and `Math.round` is `floor(x + 0.5)`, so the
        // half lands on 230. Doing the arithmetic in f64 gives 229.4999939,
        // and `f32::round`'s half-away-from-zero would give 230 from 229.5 by
        // a different rule that disagrees on negatives.
        assert_eq!(v, 230);
        let red = argb_set_brightness(0xFFFF_0000, 0.9);
        assert_eq!(red, 0xFFE6_0000);
    }

    #[test]
    fn the_style_bands_pick_the_documented_sprite() {
        let s = WaypointStyle {
            key: "minecraft:default".into(),
            near_distance: DEFAULT_NEAR_DISTANCE,
            far_distance: DEFAULT_FAR_DISTANCE,
            sprites: vec![0, 1, 2, 3],
        };
        assert_eq!(s.sprite(0.0), 0);
        assert_eq!(s.sprite(127.9), 0);
        assert_eq!(s.sprite(128.0), 1);
        assert_eq!(s.sprite(230.0), 2);
        // Just inside `farDistance` the band is still 2, **not** 3:
        // `lerpInt(alpha, 1, size - 1)` is `1 + floor(alpha * (size - 2))`, so
        // a 4-sprite style only ever reaches index 3 through the `>= far`
        // early return. Reading `lerpInt`'s `p1` as an inclusive endpoint
        // makes the last band one sprite wide and unreachable.
        assert_eq!(s.sprite(331.9), 2);
        assert_eq!(s.sprite(332.0), 3);
        assert_eq!(s.sprite(f32::INFINITY), 3);
        // The explicit 3-entry branch is not the `lerpInt` result.
        let three = WaypointStyle {
            key: "x".into(),
            near_distance: 100,
            far_distance: 200,
            sprites: vec![7, 8, 9],
        };
        assert_eq!(three.sprite(199.0), 8);
        // MISSING: near 0, far 1, one sprite — everything is "far".
        let missing = WaypointStyle {
            key: "missing".into(),
            near_distance: 0,
            far_distance: 1,
            sprites: vec![5],
        };
        assert_eq!(missing.sprite(0.5), 5);
        assert_eq!(missing.sprite(9999.0), 5);
    }

    #[test]
    fn the_contextual_bar_gives_way_to_a_recent_xp_change() {
        assert!(contextual_bar(true, false, false));
        assert!(contextual_bar(true, true, false));
        assert!(!contextual_bar(true, true, true));
        assert!(!contextual_bar(false, false, false));
    }

    #[test]
    fn the_arrow_blinks_on_a_fourteen_tick_cycle() {
        assert_eq!(arrow_frame(0), 0);
        assert_eq!(arrow_frame(9), 0);
        assert_eq!(arrow_frame(10), 1);
        assert_eq!(arrow_frame(13), 1);
        assert_eq!(arrow_frame(14), 0);
        assert_eq!(arrow_frame(-1), 1);
    }

    #[test]
    fn the_nearest_waypoint_draws_last() {
        let c = cam();
        let near = LocatorWaypoint {
            subject: WaypointSubject::Vec3i {
                x: 0,
                y: 64,
                z: 10,
                entity_eye: None,
            },
            color: 0xFF00_FF00,
            style: 0,
            is_camera_entity: false,
        };
        let far = LocatorWaypoint {
            subject: WaypointSubject::Vec3i {
                x: 0,
                y: 64,
                z: 500,
                entity_eye: None,
            },
            color: 0xFFFF_0000,
            style: 0,
            is_camera_entity: false,
        };
        let styles = [WaypointStyle {
            key: "minecraft:default".into(),
            near_distance: DEFAULT_NEAR_DISTANCE,
            far_distance: DEFAULT_FAR_DISTANCE,
            sprites: vec![0, 1, 2, 3],
        }];
        let m = markers(&[near, far], &styles, &c, 320, 240);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].color, 0xFFFF_0000, "the far dot must be emitted first");
        assert_eq!(m[1].color, 0xFF00_FF00);
        // …and the far one shows the smallest sprite.
        assert_eq!(m[0].sprite, 3);
        assert_eq!(m[1].sprite, 0);
    }

    #[test]
    fn the_camera_entitys_own_waypoint_is_skipped() {
        let c = cam();
        let styles = [WaypointStyle {
            key: "minecraft:default".into(),
            near_distance: DEFAULT_NEAR_DISTANCE,
            far_distance: DEFAULT_FAR_DISTANCE,
            sprites: vec![0, 1, 2, 3],
        }];
        let mut w = LocatorWaypoint {
            subject: WaypointSubject::Vec3i {
                x: 0,
                y: 64,
                z: 10,
                entity_eye: None,
            },
            color: 0xFFFF_FFFF,
            style: 0,
            is_camera_entity: false,
        };
        assert_eq!(markers(std::slice::from_ref(&w), &styles, &c, 320, 240).len(), 1);
        w.is_camera_entity = true;
        assert!(markers(&[w], &styles, &c, 320, 240).is_empty());
    }

    #[test]
    fn the_nine_slice_tiles_its_middle() {
        // A 12-wide source: columns 0..5 left, 5..7 the 2-px tile, 7..12 right.
        let mut rgba = vec![0u8; 12 * 1 * 4];
        for x in 0..12usize {
            rgba[x * 4] = x as u8;
        }
        let src = HudSpriteData {
            rgba: &rgba,
            w: 12,
            h: 1,
        };
        let out = expand_nine_slice(&src, 182);
        let col = |x: usize| out[x * 4];
        for x in 0..5 {
            assert_eq!(col(x), x as u8, "left border column {x}");
        }
        for i in 0..5 {
            assert_eq!(col(182 - 5 + i), (7 + i) as u8, "right border column {i}");
        }
        // The middle repeats columns 5, 6, 5, 6, … across 172 px.
        for i in 0..172usize {
            assert_eq!(col(5 + i), (5 + i % 2) as u8, "tiled column {i}");
        }
    }

    #[test]
    fn the_horizon_reads_up_when_you_look_down() {
        // Minecraft's xRot is positive downward.
        assert!(project_horizon_to_screen(0.0, 70.0).abs() < 1e-12);
        assert!(project_horizon_to_screen(60.0, 70.0) > 1.0);
        assert!(project_horizon_to_screen(-60.0, 70.0) < -1.0);
        assert_eq!(project_horizon_to_screen(90.0, 70.0), f64::INFINITY);
        assert_eq!(project_horizon_to_screen(-90.0, 70.0), f64::NEG_INFINITY);
        let mut c = cam();
        c.pitch = 60.0;
        assert_eq!(
            pitch_direction(&WaypointSubject::Chunk { x: 5, z: 5 }, &c),
            PitchDirection::Up
        );
        c.pitch = -60.0;
        assert_eq!(
            pitch_direction(&WaypointSubject::Azimuth { radians: 0.0 }, &c),
            PitchDirection::Down
        );
    }

    #[test]
    fn a_point_high_above_reads_up() {
        let c = cam();
        let s = WaypointSubject::Vec3i {
            x: 0,
            y: 200,
            z: 10,
            entity_eye: None,
        };
        assert_eq!(pitch_direction(&s, &c), PitchDirection::Up);
        let s = WaypointSubject::Vec3i {
            x: 0,
            y: -60,
            z: 10,
            entity_eye: None,
        };
        assert_eq!(pitch_direction(&s, &c), PitchDirection::Down);
        // On the crosshair — no arrow.
        let s = WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 10,
            entity_eye: None,
        };
        assert_eq!(pitch_direction(&s, &c), PitchDirection::None);
    }

    #[test]
    fn the_entity_eye_moves_the_bearing_but_not_the_distance() {
        let c = cam();
        let raw = WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 100,
            entity_eye: None,
        };
        let with_eye = WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 100,
            entity_eye: Some([20.0, 64.0, 100.0]),
        };
        assert_ne!(
            yaw_angle_to_camera(&raw, &c),
            yaw_angle_to_camera(&with_eye, &c)
        );
        // `distanceSquared` reads `this.vector`, never the entity.
        assert_eq!(distance_squared(&raw, &c), distance_squared(&with_eye, &c));
    }
}
