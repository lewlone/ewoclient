//! Compile the M0 overlay shaders GLSL -> SPIR-V with glslc (Vulkan SDK).
//! REWO_PLAN.md D4: GLSL + glslc, revisit Slang if/when mesh shaders land.

use std::{env, path::PathBuf, process::Command};

fn main() {
    let out: PathBuf = env::var("OUT_DIR").unwrap().into();
    let glslc = find_glslc();
    for (src, dst) in [
        ("shaders/overlay.vert", "overlay.vert.spv"),
        ("shaders/overlay.frag", "overlay.frag.spv"),
        ("shaders/world.vert", "world.vert.spv"),
        ("shaders/world.frag", "world.frag.spv"),
        ("shaders/water.frag", "water.frag.spv"),
        ("shaders/entity.vert", "entity.vert.spv"),
        ("shaders/entity.frag", "entity.frag.spv"),
        ("shaders/sky.vert", "sky.vert.spv"),
        ("shaders/sky.frag", "sky.frag.spv"),
        ("shaders/hud.vert", "hud.vert.spv"),
        ("shaders/hud.frag", "hud.frag.spv"),
        ("shaders/cull.comp", "cull.comp.spv"),
    ] {
        println!("cargo:rerun-if-changed={src}");
        let output = Command::new(&glslc)
            .args(["--target-env=vulkan1.3", "-O", src, "-o"])
            .arg(out.join(dst))
            .output()
            .unwrap_or_else(|e| panic!("could not run {glslc:?}: {e}"));
        if !output.status.success() {
            panic!(
                "glslc failed on {src}:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

fn find_glslc() -> String {
    if let Ok(sdk) = env::var("VULKAN_SDK") {
        let exe = if cfg!(windows) { "glslc.exe" } else { "glslc" };
        let p = PathBuf::from(&sdk).join("Bin").join(exe);
        if p.exists() {
            return p.to_string_lossy().into_owned();
        }
    }
    // Fall back to PATH; the build error names the missing tool clearly.
    "glslc".to_string()
}
