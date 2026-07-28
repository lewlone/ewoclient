//! Screenshot capture (M51) — vanilla's naming, directory and dedup ladder.
//!
//! The render half is already in `rewo-gpu`: `Offscreen` renders the whole
//! scene through the same `WorldRenderer::draw` the live client uses, reads it
//! back and encodes a PNG. What was missing is everything around it — where the
//! file goes, what it is called, and how a second capture in the same second
//! avoids clobbering the first.
//!
//! **Rewo must NOT copy vanilla's vertical flip.** `Screenshot.takeScreenshot`
//! inverts the destination row (`setPixelABGR(x, height - y - 1, …)`) because
//! GL's `glReadPixels` hands back a bottom-up image. Vulkan images are top-left
//! origin, so Rewo's readback is already the right way up and flipping it would
//! break it. Recorded here because it is the kind of line that looks like a
//! missing feature when you diff the two implementations.

use std::path::{Path, PathBuf};

/// Vanilla's screenshot filename stem: `yyyy-MM-dd_HH.mm.ss`.
///
/// `Util.getFilenameFormattedDateTime()` is
/// `DateTimeFormatter.ofPattern("yyyy-MM-dd_HH.mm.ss", Locale.ROOT)` over a
/// `ZonedDateTime.now()`. Note the separators are not symmetric — the date uses
/// `-` and the time uses `.`, because `:` is not legal in a Windows filename.
///
/// Takes the civil fields rather than reading a clock, so the naming rule is
/// testable without one. [`local_civil_now`] supplies them.
pub fn filename_stem(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}_{hour:02}.{min:02}.{sec:02}")
}

/// The first free `<stem>.png`, `<stem>_2.png`, `<stem>_3.png`, … in `dir`.
///
/// `Screenshot.getFile` counts from 1 and **omits the suffix for 1**, so the
/// first capture of a given second is bare and only a collision produces `_2`.
/// Reproduced including that asymmetry: starting the ladder at `_1` would
/// rename every ordinary screenshot.
///
/// Vanilla's loop is a non-atomic `exists()` check and so is this — two
/// captures racing in the same second can still collide, which is vanilla's
/// behaviour and not worth diverging from.
pub fn next_free_path(dir: &Path, stem: &str) -> PathBuf {
    let mut count = 1u32;
    loop {
        let name = if count == 1 {
            format!("{stem}.png")
        } else {
            format!("{stem}_{count}.png")
        };
        let path = dir.join(name);
        if !path.exists() {
            return path;
        }
        count += 1;
    }
}

/// Where captures go: `<config>/EwoClient/screenshots/`.
///
/// Vanilla uses `<gameDirectory>/screenshots`. Rewo has no game-directory
/// notion — every path in the client resolves through `dirs::config_dir()` +
/// `EwoClient` — so this is the analogue rather than the literal path. Created
/// on demand, as `Screenshot.grab` does with `picDir.mkdir()`.
pub fn screenshot_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("screenshots");
    Some(p)
}

/// Civil date-time in the **local** zone, as `(y, mo, d, h, mi, s)`.
///
/// Vanilla formats a `ZonedDateTime.now()`, i.e. the system zone. Rewo has no
/// date/time dependency, so the conversion from a Unix timestamp is here: the
/// days-from-civil algorithm, with the era arithmetic that makes it valid for
/// negative years rather than only since 1970.
///
/// **Known deviation, deliberately isolated:** the zone offset is supplied by
/// the caller and [`local_offset_seconds`] currently returns 0, so filenames
/// are UTC where vanilla's are local. The naming rule above is what the gate
/// asserts and it is exact; only the clock source is short. Kept as one
/// function so fixing it is one function.
pub fn civil_from_unix(secs: i64, offset_seconds: i32) -> (i32, u32, u32, u32, u32, u32) {
    let t = secs + offset_seconds as i64;
    // Split into whole days and the second-of-day, flooring so pre-epoch
    // instants borrow correctly instead of truncating toward zero.
    let days = t.div_euclid(86_400);
    let sod = t.rem_euclid(86_400);
    let (hour, min, sec) = (sod / 3600, (sod % 3600) / 60, sod % 60);

    // Howard Hinnant's civil_from_days: shift the epoch to 0000-03-01 so the
    // leap day lands at the end of the year and the month length pattern
    // becomes a closed form.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    // March-based month back to January-based; the year advances with it.
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32, hour as u32, min as u32, sec as u32)
}

/// The system's UTC offset in seconds.
///
/// Returns 0 — see the deviation note on [`civil_from_unix`]. Isolated so the
/// platform call can land without touching the naming rule the gate pins.
pub fn local_offset_seconds() -> i32 {
    0
}

/// `(y, mo, d, h, mi, s)` for right now.
pub fn local_civil_now() -> (i32, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_from_unix(secs, local_offset_seconds())
}

/// The path this instant's capture should be written to, creating the
/// directory. `None` when there is no config directory to resolve.
pub fn next_capture_path() -> Option<PathBuf> {
    let dir = screenshot_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let (y, mo, d, h, mi, s) = local_civil_now();
    Some(next_free_path(&dir, &filename_stem(y, mo, d, h, mi, s)))
}

/// Render the current scene to a fresh offscreen target and save it (M51).
///
/// `format` must be the live renderer's colour format — `WorldRenderer` bakes
/// it into every pipeline, so an offscreen in the gates' RGBA default would be
/// a dynamic-rendering format mismatch rather than a screenshot.
///
/// The target is built per capture rather than kept: `Offscreen` has no
/// `resize`, its extent is fixed at construction, and a window that changed
/// size between captures would otherwise write a stale-sized PNG.
pub fn grab(
    gpu: &mut rewo_gpu::Gpu,
    world: &mut rewo_gpu::world::WorldRenderer,
    view_proj: [[f32; 4]; 4],
    draw: &rewo_gpu::overlay::OverlayDraw,
    format: ash::vk::Format,
    extent: ash::vk::Extent2D,
) -> Result<PathBuf, String> {
    let (w, h) = (extent.width, extent.height);
    let path = next_capture_path().ok_or("no config directory for screenshots")?;
    let mut off = rewo_gpu::offscreen::Offscreen::with_format(gpu, w, h, format)?;
    let res = (|| {
        off.render(gpu, Some((world, view_proj)), draw, CLEAR)?;
        off.save_png(gpu, &path)
    })();
    // Destroy either way — a failed capture must not leak an image, an
    // allocation and a command pool per keypress.
    off.destroy(gpu);
    res.map(|_| path)
}

/// The world pass clears to opaque black before the sky paints over it; a
/// capture wants the same clear the live frame uses.
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_matches_vanillas_pattern() {
        // `-` between date parts, `.` between time parts — not symmetric.
        assert_eq!(filename_stem(2026, 7, 28, 9, 4, 5), "2026-07-28_09.04.05");
    }

    #[test]
    fn epoch_round_trips() {
        assert_eq!(civil_from_unix(0, 0), (1970, 1, 1, 0, 0, 0));
        // A leap day, to exercise the era arithmetic rather than a lucky offset.
        assert_eq!(civil_from_unix(1_709_164_800, 0), (2024, 2, 29, 0, 0, 0));
        // Pre-epoch: floors rather than truncating toward zero.
        assert_eq!(civil_from_unix(-1, 0), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn offset_shifts_the_civil_day() {
        // 23:30 UTC plus an hour is the next day, not hour 24.
        assert_eq!(civil_from_unix(84_600, 3600), (1970, 1, 2, 0, 30, 0));
    }

    #[test]
    fn dedup_ladder_omits_the_suffix_for_the_first() {
        let dir = std::env::temp_dir().join(format!("rewo-capture-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stem = "2026-07-28_09.04.05";

        // First is bare — starting the ladder at `_1` would rename every
        // ordinary screenshot, which is the mutation this guards.
        let first = next_free_path(&dir, stem);
        assert_eq!(first.file_name().unwrap(), "2026-07-28_09.04.05.png");
        std::fs::write(&first, b"x").unwrap();

        // Only a collision produces a suffix, and it starts at 2.
        let second = next_free_path(&dir, stem);
        assert_eq!(second.file_name().unwrap(), "2026-07-28_09.04.05_2.png");
        std::fs::write(&second, b"x").unwrap();

        let third = next_free_path(&dir, stem);
        assert_eq!(third.file_name().unwrap(), "2026-07-28_09.04.05_3.png");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
