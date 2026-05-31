//! Embeds the Windows app icon into the `ewolauncher.exe` so it shows in the
//! taskbar, Explorer, and any shortcut.
//!
//! Drop a multi-resolution `assets/icon.ico` at the workspace root and rebuild
//! (`cargo build --release -p ewo-launcher`). Until that file exists this is a
//! no-op, so the build never blocks on the art.

fn main() {
    #[cfg(target_os = "windows")]
    {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        // crates/ewo-launcher -> workspace root -> assets/icon.ico
        let icon = std::path::Path::new(&manifest)
            .join("..")
            .join("..")
            .join("assets")
            .join("icon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        if icon.exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(&icon.to_string_lossy());
            if let Err(e) = res.compile() {
                // Don't fail the build over the icon — just warn.
                println!("cargo:warning=app-icon embed failed: {e}");
            }
        }
    }
}
