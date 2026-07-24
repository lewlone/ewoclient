//! Player-skin resolution + PNG fetch (shared by `mobshot --skin` and the
//! live session's async skin loader).
//!
//! `resolve` turns a username or a raw texture URL into `(url, slim)`;
//! `fetch_rgba64` downloads the PNG and normalizes it to a 64×64 RGBA
//! buffer (the entity atlas's skin-slot size). Legacy 64×32 skins are
//! top-anchored with a transparent lower half (nearly extinct — a warn,
//! not an expansion).

use rewo_net::skins::SkinInfo;

/// Resolve `name_or_url` to a skin URL + slim flag. A value starting with
/// `http` is used verbatim (model unknown → classic); otherwise it's a
/// Minecraft username, resolved via Mojang's profile API.
pub fn resolve(name_or_url: &str) -> Result<SkinInfo, String> {
    if name_or_url.starts_with("http") {
        return Ok(SkinInfo {
            url: name_or_url.to_string(),
            slim: false,
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
        .ok_or_else(|| "profile: no skin in textures".into())
}

/// Download a skin PNG and normalize it to a 64×64×4 RGBA buffer.
pub fn fetch_rgba64(url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    ureq::get(url)
        .call()
        .map_err(|e| format!("skin fetch {url}: {e}"))?
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("skin read: {e}"))?;
    decode_png_to_64(&bytes)
}

/// Decode PNG bytes → 64×64 RGBA. Handles RGB (opaque) and RGBA sources,
/// palette/grayscale via EXPAND; 64×32 legacy skins land in the top half.
fn decode_png_to_64(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("skin png: {e}"))?;
    let bufsize = reader
        .output_buffer_size()
        .ok_or("skin png: bad buffer size")?;
    let mut buf = vec![0u8; bufsize];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("skin frame: {e}"))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("skin png still indexed after EXPAND".into()),
    };
    if w != 64 || (h != 64 && h != 32) {
        return Err(format!(
            "skin png is {w}×{h}, expected 64×64 (or legacy 64×32)"
        ));
    }
    if h == 32 {
        log::warn!("skin_fetch: legacy 64×32 skin — lower body/arm faces will be transparent");
    }
    let mut out = vec![0u8; 64 * 64 * 4];
    for y in 0..h {
        for x in 0..64 {
            let src = (y * w + x) * channels;
            let dst = (y * 64 + x) * 4;
            let (r, g, b, a) = match channels {
                4 => (buf[src], buf[src + 1], buf[src + 2], buf[src + 3]),
                3 => (buf[src], buf[src + 1], buf[src + 2], 255),
                2 => (buf[src], buf[src], buf[src], buf[src + 1]),
                _ => (buf[src], buf[src], buf[src], 255),
            };
            out[dst..dst + 4].copy_from_slice(&[r, g, b, a]);
        }
    }
    Ok(out)
}

use std::io::Read as _;
