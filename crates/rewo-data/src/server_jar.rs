//! Ensure the vanilla server jar (for running the data generator) exists on
//! disk, sha1-verified. Reuses the launcher's convention: the per-version
//! manifest's `downloads.server.{url,sha1}`.

use std::path::Path;

use sha1::{Digest, Sha1};

/// Download `url` to `dest` if missing/mismatched, verifying `sha1_hex`.
pub fn ensure(dest: &Path, url: &str, sha1_hex: &str) -> Result<(), String> {
    if dest.exists() && file_sha1(dest).ok().as_deref() == Some(sha1_hex) {
        return Ok(());
    }
    log::info!("rewo-data: downloading server jar from {url}");
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("server jar GET: {e}"))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("server jar read: {e}"))?;
    let got = hex(&Sha1::digest(&bytes));
    if got != sha1_hex {
        return Err(format!("server jar sha1 {got} != expected {sha1_hex}"));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    std::fs::write(dest, &bytes).map_err(|e| format!("write server jar: {e}"))?;
    Ok(())
}

fn file_sha1(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    Ok(hex(&Sha1::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
