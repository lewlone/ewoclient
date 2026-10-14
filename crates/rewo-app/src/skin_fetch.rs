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
use std::io::Read as _;
use std::time::Duration;

// # Hostile-server hardening (audit 2026-10 H2)
//
// The texture URLs come from a server-controlled `textures` property, so every
// fetch here treats the URL, the response and the PNG header as untrusted:
// the URL must pass vanilla's domain rule ([`rewo_net::skins::allowed_texture_url`]),
// the connection has timeouts and no redirects, the body is capped, and the PNG
// dimensions are read from the header and checked before any pixel buffer is
// allocated.

/// Hard cap on a downloaded body. A 64x64 skin is ~10 KiB and a 1024x512 cape
/// well under 1 MiB; anything larger is hostile or broken.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Cap for the Mojang profile JSON responses.
const MAX_JSON_BYTES: usize = 256 * 1024;
/// A skin is exactly 64 wide and 64 (or legacy 32) tall
/// (`SkinTextureDownloader.processLegacySkin`, lines 101-137).
const SKIN_MAX_DIMS: (u32, u32) = (64, 64);
/// Vanilla does not size-check capes. Rewo box-downsamples integer multiples of
/// 64x32 into its slot, so the cap is 16x (1024x512), 2 MiB of RGBA.
const CAPE_MAX_DIMS: (u32, u32) = (1024, 512);

/// The one HTTP agent for skin traffic: connect + read + overall timeouts and
/// no redirects (a redirect would sidestep the URL check).
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(10))
            // Bounds a server that trickles bytes just under the read timeout.
            .timeout(Duration::from_secs(20))
            .redirects(0)
            .build()
    })
}

/// Read at most `cap` bytes from `r`; one byte more is an error, not a
/// truncation (a truncated PNG would only fail later, less clearly).
fn read_capped(r: impl std::io::Read, cap: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    r.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read: {e}"))?;
    if bytes.len() > cap {
        return Err(format!("body exceeds the {cap}-byte cap"));
    }
    Ok(bytes)
}

/// GET `url` on the shared agent: status must be 200, body capped at `cap`.
fn bounded_get(url: &str, cap: usize) -> Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| format!("{e}"))?;
    if resp.status() != 200 {
        // Redirects are off, so a 3xx lands here too.
        return Err(format!("HTTP {}", resp.status()));
    }
    if resp
        .header("Content-Length")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .is_some_and(|n| n > cap)
    {
        return Err(format!("Content-Length exceeds the {cap}-byte cap"));
    }
    read_capped(resp.into_reader(), cap)
}

/// Fetch a texture URL: re-validate it (defence in depth, the decoder already
/// did), upgrade `http` to `https` (Mojang's session server still hands out
/// `http://textures.minecraft.net/...`, which serves the same bytes over TLS)
/// and download with the body cap.
fn http_get(url: &str, what: &str) -> Result<Vec<u8>, String> {
    let url = rewo_net::skins::allowed_texture_url(url)
        .ok_or_else(|| format!("{what}: refusing texture URL outside the vanilla allow-list"))?;
    let fetch = match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    };
    bounded_get(&fetch, MAX_BODY_BYTES).map_err(|e| format!("{what} fetch {url}: {e}"))
}

/// A Minecraft username: 1..=16 of `[A-Za-z0-9_]`. (Mojang issues 3..=16;
/// shorter names just come back "unknown".)
fn valid_username(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Percent-encode everything but RFC 3986 unreserved characters, so a value
/// can never alter the path it is spliced into.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Resolve `name_or_url` to a skin URL + slim flag. A value starting with
/// `http` is a raw texture URL (model unknown -> classic) and must pass the
/// vanilla domain rule; otherwise it's a Minecraft username, resolved via
/// Mojang's profile API.
pub fn resolve(name_or_url: &str) -> Result<SkinInfo, String> {
    if name_or_url.starts_with("http") {
        let url = rewo_net::skins::allowed_texture_url(name_or_url)
            .ok_or("resolve: texture URL is outside the vanilla allow-list")?;
        return Ok(SkinInfo {
            url: Some(url.to_string()),
            slim: false,
            cape: None,
            elytra: None,
        });
    }
    if !valid_username(name_or_url) {
        return Err(format!("resolve: {name_or_url:?} is not a valid username"));
    }
    // username -> uuid
    let url = format!(
        "https://api.mojang.com/users/profiles/minecraft/{}",
        pct_encode(name_or_url)
    );
    let json = get_json(&url).map_err(|e| format!("resolve {name_or_url}: {e}"))?;
    let id = json["id"]
        .as_str()
        .ok_or("resolve: no id (unknown username?)")?;
    // The id is spliced into the next URL: it must be a bare 32-digit UUID.
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("resolve: malformed profile id".into());
    }
    // uuid -> profile -> textures property
    let purl = format!("https://sessionserver.mojang.com/session/minecraft/profile/{id}");
    let profile = get_json(&purl).map_err(|e| format!("profile {id}: {e}"))?;
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

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let body = bounded_get(url, MAX_JSON_BYTES)?;
    serde_json::from_slice(&body).map_err(|e| format!("json: {e}"))
}

/// Download a skin PNG and normalize it to a 64×64×4 RGBA buffer.
pub fn fetch_rgba64(url: &str) -> Result<Vec<u8>, String> {
    decode_png_to_64(&http_get(url, "skin")?)
}

/// Download a cape PNG and normalize it to the atlas's 64×32×4 cape slot.
pub fn fetch_cape_rgba(url: &str) -> Result<Vec<u8>, String> {
    decode_cape(&http_get(url, "cape")?)
}

/// Read a PNG's `(width, height)` straight from its IHDR chunk (signature, then
/// the first chunk, which the spec requires to be IHDR) **without** touching the
/// decoder or allocating, and reject anything larger than `max` in either axis
/// or empty. Parsing it here also means the verdict does not depend on the
/// stream carrying any IDAT: a bare header already fails.
fn png_dims_within(bytes: &[u8], max: (u32, u32)) -> Result<(u32, u32), String> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return Err("png: missing signature or IHDR".into());
    }
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (w, h) = (be(16), be(20));
    if w == 0 || h == 0 || w > max.0 || h > max.1 {
        return Err(format!(
            "png is {w}×{h}, larger than the {}×{} limit",
            max.0, max.1
        ));
    }
    Ok((w, h))
}

/// Decode PNG bytes → `(rgba, w, h)` at the source's own size, refusing
/// (before any allocation) a header larger than `max_dims`.
///
/// Handles RGB (opaque), RGBA, grayscale and palette (via EXPAND) sources.
/// The caller owns every size decision.
pub fn decode_png_rgba(
    bytes: &[u8],
    max_dims: (u32, u32),
) -> Result<(Vec<u8>, usize, usize), String> {
    png_dims_within(bytes, max_dims)?;
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    // Belt and braces: even a header inside `max_dims` may not allocate more
    // than a few MiB (the default limit is 64 MiB).
    dec.set_limits(png::Limits {
        bytes: 8 * 1024 * 1024,
    });
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
    let (src, w, h) = decode_png_rgba(bytes, SKIN_MAX_DIMS)?;
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
    let (src, w, h) = decode_png_rgba(bytes, CAPE_MAX_DIMS)?;
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

    // ---- hostile-server hardening (audit 2026-10 H2) ----------------------

    /// A PNG that is nothing but a signature and an IHDR claiming `w`×`h`:
    /// no IDAT, so anything that tried to decode it (or allocate from the
    /// header) would fail differently from the dimension check.
    fn header_only_png(w: u32, h: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        out.extend_from_slice(&13u32.to_be_bytes());
        out.extend_from_slice(&ihdr);
        out.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        out
    }

    #[test]
    fn a_header_claiming_100000_square_is_rejected_before_any_allocation() {
        let bytes = header_only_png(100_000, 100_000);
        for err in [
            decode_png_to_64(&bytes).unwrap_err(),
            decode_cape(&bytes).unwrap_err(),
            png_dims_within(&bytes, (1024, 512)).unwrap_err(),
        ] {
            // The dimension check's own message, not a decode failure from
            // the missing IDAT: the header was judged first.
            assert!(err.contains("larger than"), "{err}");
        }
        // A sane header passes the check (the decode then fails: no IDAT).
        assert_eq!(
            png_dims_within(&header_only_png(64, 64), (64, 64)),
            Ok((64, 64))
        );
        assert!(
            png_dims_within(&header_only_png(0, 64), (64, 64)).is_err(),
            "empty"
        );
    }

    #[test]
    fn dimension_limits_follow_the_asset_kind() {
        assert!(decode_png_to_64(&png(65, 64, |_, _| [0; 4])).is_err());
        assert!(decode_png_to_64(&png(64, 65, |_, _| [0; 4])).is_err());
        // 16x cape is the ceiling; 32x is refused on the header alone.
        assert!(decode_cape(&png(1024, 512, |_, _| [1, 2, 3, 255])).is_ok());
        let err = decode_cape(&header_only_png(2048, 1024)).unwrap_err();
        assert!(err.contains("larger than"), "{err}");
    }

    #[test]
    fn a_body_one_byte_over_the_cap_is_rejected() {
        let cap = 1000;
        assert_eq!(
            read_capped(std::io::repeat(7).take(cap as u64), cap)
                .unwrap()
                .len(),
            cap
        );
        let err = read_capped(std::io::repeat(7).take(cap as u64 + 1), cap).unwrap_err();
        assert!(err.contains("cap"), "{err}");
        // An endless body is cut off at cap+1 rather than read to exhaustion.
        assert!(read_capped(std::io::repeat(7), cap).is_err());
        assert_eq!(read_capped(std::io::empty(), cap).unwrap().len(), 0);
    }

    #[test]
    fn a_disallowed_url_is_refused_before_any_connection() {
        for u in [
            "http://127.0.0.1:1/x",
            "https://example.com/x.png",
            "https://textures.minecraft.net.evil.com/x",
            "https://textures.minecraft.net@evil.com/x",
        ] {
            let err = http_get(u, "skin").unwrap_err();
            assert!(err.contains("allow-list"), "{u}: {err}");
            assert!(fetch_rgba64(u).is_err());
            assert!(fetch_cape_rgba(u).is_err());
        }
        assert!(resolve("http://127.0.0.1/s.png").is_err());
    }

    #[test]
    fn usernames_are_validated_and_encoded() {
        assert!(valid_username("lewlone"));
        assert!(valid_username("Name_12"));
        for bad in [
            "",
            "seventeen_chars_x",
            "a/b",
            "a b",
            "a?b",
            "..",
            "x%2fy",
            "näme",
            "a\n",
        ] {
            assert!(!valid_username(bad), "{bad:?}");
            assert!(resolve(bad).is_err(), "{bad:?}");
        }
        assert_eq!(pct_encode("Name_1-.~"), "Name_1-.~");
        assert_eq!(pct_encode("a/b?c#d e"), "a%2Fb%3Fc%23d%20e");
    }
}
