//! Runtime window icon — feeds `assets/icon.ico` into winit's
//! `with_window_icon` so the borderless window's taskbar / alt-tab /
//! title-bar entries show the app mark instead of the OS default.
//!
//! The exe *embeds* the same .ico via `build.rs` / `winresource`, but
//! that copy only serves Explorer and shortcuts; a borderless winit
//! window must be handed the pixels itself.
//!
//! Container rules implemented here (the `.ico` spec, not a guess):
//! a 6-byte `ICONDIR` followed by N 16-byte directory entries whose
//! payloads are either 256+-byte DIBs (`0x28` header) or plain PNGs.
//! Entries are *usually* listed smallest-first, but nothing requires
//! it — we scan all of them and take the largest PNG payload, which is
//! what a taskbar wants anyway (winit scales down cleanly).

use winit::window::Icon;

/// First four bytes of a PNG stream — how an `.ico` entry declares
/// itself PNG-encoded rather than DIB.
const PNG_SIG: [u8; 4] = [0x89, b'P', b'N', b'G'];

/// Locate `assets/icon.ico` next to the executable first (portable
/// builds), falling back to the compile-time workspace copy so
/// `cargo run` doesn't need an assets folder in `target/`.
fn locate_icon() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let next_to_exe = dir.join("assets").join("icon.ico");
            if next_to_exe.is_file() {
                return Some(next_to_exe);
            }
        }
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut p = std::path::PathBuf::from(manifest);
    p.pop(); // crates/
    p.pop(); // workspace root
    p.push("assets");
    p.push("icon.ico");
    p.is_file().then_some(p)
}

/// Extract the largest PNG-encoded entry from an `.ico` byte blob.
/// Returns the raw PNG bytes (still compressed — decoded by the caller).
/// Non-PNG (BMP/DIB) entries are skipped, matching this project's own
/// asset pipeline which always includes a 256px PNG frame.
pub fn largest_png_entry(ico: &[u8]) -> Option<&[u8]> {
    // ICONDIR: reserved u16 (0), type u16 (1 = icon), count u16.
    if ico.len() < 6 || ico[2..4] != [1, 0] {
        return None;
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut best: Option<(usize, usize, usize)> = None; // (bytes, off, len)
    for i in 0..count {
        let e = 6 + i * 16;
        if e + 16 > ico.len() {
            return None;
        }
        let size = u32::from_le_bytes([ico[e + 8], ico[e + 9], ico[e + 10], ico[e + 11]])
            as usize;
        let off =
            u32::from_le_bytes([ico[e + 12], ico[e + 13], ico[e + 14], ico[e + 15]])
                as usize;
        if size == 0 || off.saturating_add(size) > ico.len() {
            continue;
        }
        let bigger = best.map(|(b, _, _)| size > b).unwrap_or(true);
        if ico[off..off + 4] == PNG_SIG && bigger {
            best = Some((size, off, size));
        }
    }
    let (_, off, len) = best?;
    Some(&ico[off..off + len])
}

/// Decode PNG bytes into a premultiplied-free RGBA8 buffer for winit.
/// Accepts 8-bit RGB(A); anything else (paletted, 16-bit) fails to
/// `None` rather than producing a wrong-looking icon — though the
/// project's own pipeline emits straight RGBA 32-bit.
/// Decode PNG bytes into a straight-alpha RGBA8 buffer for winit.
/// Accepts 8-bit RGB(A) plus paletted-with-PLTE; anything else fails
/// with a readable reason rather than producing a wrong-looking icon.
fn rgba_from_png(png_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("png header: {e}"))?;
    let (w, h) = (reader.info().width, reader.info().height);
    let buf_len = reader
        .output_buffer_size()
        .ok_or_else(|| "png: no output buffer size (unsupported transform combo)".to_string())?;
    let mut out = vec![0u8; buf_len];
    let frame = reader.next_frame(&mut out).map_err(|e| format!("png data: {e}"))?;
    out.truncate(frame.buffer_size());
    match frame.color_type {
        png::ColorType::Rgba => Ok((out, w, h)),
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity(out.len() / 3 * 4);
            for px in out.as_chunks::<3>().0 {
                rgba.extend_from_slice(px);
                rgba.push(255);
            }
            Ok((rgba, w, h))
        }
        // Paletted icons: map through PLTE (+ tRNS when present).
        png::ColorType::Indexed => {
            let info = reader.info();
            let palette = info
                .palette
                .as_ref()
                .ok_or_else(|| "indexed png without a PLTE".to_string())?;
            let trns = info.trns.as_deref();
            let mut rgba = Vec::with_capacity(out.len() * 4);
            for idx in &out {
                let base = *idx as usize * 3;
                if base + 2 >= palette.len() {
                    return Err("palette index out of range".into());
                }
                rgba.extend_from_slice(&palette[base..base + 3]);
                rgba.push(
                    trns.and_then(|t| t.get(*idx as usize).copied()).unwrap_or(255),
                );
            }
            Ok((rgba, w, h))
        }
        other => Err(format!("unsupported colour type {other:?}")),
    }
}

/// Load the app icon for the main window. `None` on any failure — the
/// window still opens with the OS default icon rather than failing the
/// launch over decoration.
pub fn load_app_icon() -> Option<Icon> {
    let path = locate_icon()?;
    let raw = std::fs::read(path).ok()?;
    let png_bytes = largest_png_entry(&raw)?;
    match rgba_from_png(png_bytes) {
        Ok((data, w, h)) => Icon::from_rgba(data, w, h).ok(),
        Err(reason) => {
            log::warn!("app icon found but undecodable: {reason}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entries declared explicitly `(w, h, size, is_png)`; payloads are
    /// laid out back-to-back after the directory table.
    fn ico_container(entries: &[(u8, u8, u32, bool)]) -> Vec<u8> {
        let mut v = vec![0, 0, 1, 0];
        v.extend((entries.len() as u16).to_le_bytes());
        let payload_base = 6 + entries.len() * 16;
        let mut cursor = payload_base as u32;
        for &(w, h, size, _png) in entries {
            v.extend_from_slice(&[w, h, 0, 0]);
            v.extend_from_slice(&[0, 0, 0, 0]); // planes/bpp unused here
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&cursor.to_le_bytes());
            cursor += size;
        }
        debug_assert_eq!(v.len(), payload_base);
        for &(_, _, size, png) in entries {
            if png {
                v.extend_from_slice(&PNG_SIG);
                v.extend(vec![0xAB; size.saturating_sub(4) as usize]);
            } else {
                v.extend_from_slice(&0x28u32.to_le_bytes()); // DIB header
                v.extend(vec![0xCD; size.saturating_sub(4) as usize]);
            }
        }
        v
    }

    #[test]
    fn picks_the_largest_png_entry_not_the_first() {
        let small_png = 64usize;
        let big_png = 4096usize;
        let bmp_entry = 1024usize;
        let blob = ico_container(&[
            (16, 16, small_png as u32, true),
            (48, 48, bmp_entry as u32, false),
            (0, 0, big_png as u32, true),
        ]);
        let got = largest_png_entry(&blob).expect("png entry found");
        assert_eq!(got.len(), big_png);
        assert_eq!(&got[..4], &PNG_SIG);
    }

    #[test]
    fn dib_only_icon_yields_nothing() {
        let blob = ico_container(&[(16, 16, 512, false), (48, 48, 2048, false)]);
        assert!(largest_png_entry(&blob).is_none());
    }

    #[test]
    fn truncated_entry_is_skipped_not_panicked_on() {
        let mut blob = ico_container(&[(0, 0, 128, true)]);
        blob.truncate(blob.len() - 100); // corrupt past the last payload end
        // Directory says 128 bytes; the blob is short now → skipped.
        assert!(largest_png_entry(&blob).is_none());
    }

    #[test]
    fn the_workspace_icon_decodes_to_rgba() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.pop();
        p.push("assets");
        p.push("icon.ico");
        let raw = std::fs::read(&p).unwrap_or_else(|e| panic!("workspace icon missing: {e}"));
        let png_bytes = largest_png_entry(&raw).expect("real ico carries a png frame");
        let (data, w, h) =
            rgba_from_png(png_bytes).unwrap_or_else(|e| panic!("decode failed: {e}"));
        assert_eq!((w, h), (256, 256));
        assert_eq!(data.len(), (w * h * 4) as usize);
        assert!(data.as_chunks::<4>().0.iter().any(|px| px[3] == 255));
    }
}
