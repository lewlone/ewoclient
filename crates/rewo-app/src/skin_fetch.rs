//! Player skin + cape resolution and PNG fetch (shared by `mobshot --skin`,
//! `capeshot` and the live session's async loaders).
//!
//! `resolve` turns a username or a raw texture URL into a [`SkinInfo`];
//! `fetch_rgba64` downloads a skin PNG and normalizes it to a 64×64 RGBA
//! buffer (the entity atlas's skin-slot size), and `fetch_cape_rgba`
//! does the same for a cape at 64×32. Legacy 64×32 skins are top-anchored
//! with a transparent lower half (nearly extinct — a warn, not an
//! expansion).
//!
//! # Why the decode is size-preserving now (M60)
//!
//! `decode_png_rgba` returns the source's own dimensions and the callers
//! decide what to do with them. Before M60 the decoder hard-coded a 64×64
//! output and rejected everything that was not 64 wide, which is right for a
//! skin and wrong for a cape: Mojang's own capes are 64×32, and third-party
//! ones are routinely 128×64 or 512×256. Those are exact power-of-two
//! multiples of the cape's UV space, so they box-downsample to it losslessly
//! in the only sense that matters here — every destination texel is the mean
//! of a whole source block, with no resampling across a UV seam.

use rewo_net::skins::SkinInfo;

/// Resolve `name_or_url` to a skin URL + slim flag. A value starting with
/// `http` is used verbatim (model unknown → classic); otherwise it's a
/// Minecraft username, resolved via Mojang's profile API.
pub fn resolve(name_or_url: &str) -> Result<SkinInfo, String> {
    if name_or_url.starts_with("http") {
        return Ok(SkinInfo {
            url: Some(name_or_url.to_string()),
            slim: false,
            cape: None,
            elytra: None,
        });
    }
    // username → uuid
    let url = format!("https://api.mojang.com/users/profiles/minecraft/{name_or_url}");
    let json: serde_json::Value = ureq::get(&url)
        .call()
        .map_err(|e| format!("resolve {name_or_url}: {e}"))?
        .into_json()
        .map_err(|e| format!("resolve json: {e}"))?;
    let id = json["id"]
        .as_str()
        .ok_or("resolve: no id (unknown username?)")?;
    // uuid → profile → textures property
    let purl = format!("https://sessionserver.mojang.com/session/minecraft/profile/{id}");
    let profile: serde_json::Value = ureq::get(&purl)
        .call()
        .map_err(|e| format!("profile {id}: {e}"))?
        .into_json()
        .map_err(|e| format!("profile json: {e}"))?;
    let props = profile["properties"]
        .as_array()
        .ok_or("profile: no properties")?;
    let tex = props
        .iter()
        .find(|p| p["name"].as_str() == Some("textures"))
        .and_then(|p| p["value"].as_str())
        .ok_or("profile: no textures property")?;
    rewo_net::skins::decode_textures_property(tex)
        .ok_or_else(|| "profile: no textures in property".into())
}

fn http_get(url: &str, what: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    ureq::get(url)
        .call()
        .map_err(|e| format!("{what} fetch {url}: {e}"))?
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{what} read: {e}"))?;
    Ok(bytes)
}

/// Download a skin PNG and normalize it to a 64×64×4 RGBA buffer.
pub fn fetch_rgba64(url: &str) -> Result<Vec<u8>, String> {
    decode_png_to_64(&http_get(url, "skin")?)
}

/// Download a cape PNG and normalize it to the atlas's 64×32×4 cape slot.
pub fn fetch_cape_rgba(url: &str) -> Result<Vec<u8>, String> {
    decode_cape(&http_get(url, "cape")?)
}

/// Decode PNG bytes → `(rgba, w, h)` at the source's own size.
///
/// Handles RGB (opaque), RGBA, grayscale and palette (via EXPAND) sources.
/// The caller owns every size decision.
pub fn decode_png_rgba(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("png: {e}"))?;
    let bufsize = reader.output_buffer_size().ok_or("png: bad buffer size")?;
    let mut buf = vec![0u8; bufsize];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png frame: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("png still indexed after EXPAND".into()),
    };
    let mut out = vec![0u8; w * h * 4];
    for i in 0..w * h {
        let src = i * channels;
        let px = match channels {
            4 => [buf[src], buf[src + 1], buf[src + 2], buf[src + 3]],
            3 => [buf[src], buf[src + 1], buf[src + 2], 255],
            2 => [buf[src], buf[src], buf[src], buf[src + 1]],
            _ => [buf[src], buf[src], buf[src], 255],
        };
        out[i * 4..i * 4 + 4].copy_from_slice(&px);
    }
    Ok((out, w, h))
}

/// Decode PNG bytes → 64×64 RGBA; 64×32 legacy skins land in the top half.
fn decode_png_to_64(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let (src, w, h) = decode_png_rgba(bytes)?;
    if w != 64 || (h != 64 && h != 32) {
        return Err(format!(
            "skin png is {w}×{h}, expected 64×64 (or legacy 64×32)"
        ));
    }
    if h == 32 {
        log::warn!("skin_fetch: legacy 64×32 skin — lower body/arm faces will be transparent");
    }
    let mut out = vec![0u8; 64 * 64 * 4];
    let copied = h * 64 * 4;
    out[..copied].copy_from_slice(&src[..copied]);
    Ok(out)
}

/// Decode PNG bytes → the atlas's **64×32** cape slot.
///
/// A cape is 2:1 and 64×32 in vanilla; higher-resolution ones are exact
/// power-of-two multiples, so an `n×n` box mean lands each destination texel
/// on a whole source block. Anything that is not such a multiple is rejected
/// rather than stretched — a wrongly-scaled cape is a subtly wrong render,
/// and a loud failure is easier to act on.
pub fn decode_cape(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let (src, w, h) = decode_png_rgba(bytes)?;
    const CW: usize = 64;
    const CH: usize = 32;
    if w < CW || h < CH || w % CW != 0 || h % CH != 0 || w / CW != h / CH {
        return Err(format!(
            "cape png is {w}×{h}, expected 64×32 or an integer square multiple of it"
        ));
    }
    let n = w / CW;
    let mut out = vec![0u8; CW * CH * 4];
    for y in 0..CH {
        for x in 0..CW {
            // Mean over the n×n source block, per channel. Straight (not
            // premultiplied) average: a cape's alpha is effectively binary,
            // and vanilla does no filtering here at all — this exists only
            // to fit an oversized sheet into the slot.
            let mut acc = [0u32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let i = ((y * n + sy) * w + (x * n + sx)) * 4;
                    for (a, c) in acc.iter_mut().zip(&src[i..i + 4]) {
                        *a += *c as u32;
                    }
                }
            }
            let d = (y * CW + x) * 4;
            let area = (n * n) as u32;
            for (k, a) in acc.iter().enumerate() {
                out[d + k] = (a / area) as u8;
            }
        }
    }
    if n != 1 {
        log::info!("skin_fetch: cape {w}×{h} downsampled {n}× into the 64×32 slot");
    }
    Ok(out)
}

use std::io::Read as _;

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a solid-colour RGBA PNG, so the decoders can be exercised
    /// without a network.
    fn png(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut raw = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                raw.extend_from_slice(&px(x, y));
            }
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(std::io::Cursor::new(&mut out), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&raw).unwrap();
        }
        out
    }

    #[test]
    fn a_vanilla_sized_cape_passes_through_untouched() {
        let bytes = png(64, 32, |x, _| [x as u8, 7, 9, 255]);
        let out = decode_cape(&bytes).unwrap();
        assert_eq!(out.len(), 64 * 32 * 4);
        assert_eq!(&out[..4], &[0, 7, 9, 255]);
        assert_eq!(&out[63 * 4..63 * 4 + 4], &[63, 7, 9, 255]);
    }

    /// The case the pre-M60 decoder rejected outright.
    #[test]
    fn a_512x256_cape_box_downsamples_into_the_slot() {
        // Each 8x8 block is a constant, so the mean is exact and a wrong
        // block size would show up immediately.
        let bytes = png(512, 256, |x, y| [(x / 8) as u8, (y / 8) as u8, 3, 255]);
        let out = decode_cape(&bytes).unwrap();
        assert_eq!(out.len(), 64 * 32 * 4);
        for y in 0..32usize {
            for x in 0..64usize {
                let d = (y * 64 + x) * 4;
                assert_eq!(&out[d..d + 4], &[x as u8, y as u8, 3, 255], "at {x},{y}");
            }
        }
    }

    #[test]
    fn a_non_multiple_cape_is_rejected_rather_than_stretched() {
        assert!(decode_cape(&png(64, 64, |_, _| [0; 4])).is_err(), "1:1");
        assert!(decode_cape(&png(96, 48, |_, _| [0; 4])).is_err(), "1.5x");
        assert!(decode_cape(&png(32, 16, |_, _| [0; 4])).is_err(), "smaller");
    }

    /// The skin path must be unchanged by the size-preserving refactor.
    #[test]
    fn skins_still_normalize_the_way_they_did() {
        let full = png(64, 64, |x, y| [x as u8, y as u8, 1, 255]);
        let out = decode_png_to_64(&full).unwrap();
        assert_eq!(out.len(), 64 * 64 * 4);
        let d = (63 * 64 + 63) * 4;
        assert_eq!(&out[d..d + 4], &[63, 63, 1, 255]);

        let legacy = png(64, 32, |x, y| [x as u8, y as u8, 1, 255]);
        let out = decode_png_to_64(&legacy).unwrap();
        assert_eq!(out.len(), 64 * 64 * 4, "still a 64x64 buffer");
        let top = (31 * 64 + 5) * 4;
        assert_eq!(&out[top..top + 4], &[5, 31, 1, 255]);
        let bottom = (32 * 64) * 4;
        assert_eq!(&out[bottom..bottom + 4], &[0, 0, 0, 0], "lower half blank");

        assert!(decode_png_to_64(&png(128, 128, |_, _| [0; 4])).is_err());
    }
}
