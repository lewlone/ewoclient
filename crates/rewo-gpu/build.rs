//! Compile the M0 overlay shaders GLSL -> SPIR-V with glslc (Vulkan SDK).
//! REWO_PLAN.md D4: GLSL + glslc, revisit Slang if/when mesh shaders land.

use std::{
    env,
    path::PathBuf,
    process::Command,
};

fn main() {
    let out: PathBuf = env::var("OUT_DIR").unwrap().into();
    let glslc = find_glslc();
    for (src, dst) in [
        ("shaders/border.vert", "border.vert.spv"),
        ("shaders/border.frag", "border.frag.spv"),
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
        ("shaders/locator.vert", "locator.vert.spv"),
        ("shaders/locator.frag", "locator.frag.spv"),
        ("shaders/line.vert", "line.vert.spv"),
        ("shaders/line.frag", "line.frag.spv"),
        ("shaders/celestial.vert", "celestial.vert.spv"),
        ("shaders/celestial.frag", "celestial.frag.spv"),
        ("shaders/sunrise.vert", "sunrise.vert.spv"),
        ("shaders/sunrise.frag", "sunrise.frag.spv"),
        ("shaders/container.vert", "container.vert.spv"),
        ("shaders/container.frag", "container.frag.spv"),
        ("shaders/hand.vert", "hand.vert.spv"),
        ("shaders/hand.frag", "hand.frag.spv"),
        ("shaders/gui_item.vert", "gui_item.vert.spv"),
        ("shaders/gui_item.frag", "gui_item.frag.spv"),
        ("shaders/gui_glint.frag", "gui_glint.frag.spv"),
        ("shaders/entity_glint.frag", "entity_glint.frag.spv"),
        ("shaders/clouds.vert", "clouds.vert.spv"),
        ("shaders/particle.vert", "particle.vert.spv"),
        ("shaders/particle.frag", "particle.frag.spv"),
        ("shaders/crumbling.vert", "crumbling.vert.spv"),
        ("shaders/crumbling.frag", "crumbling.frag.spv"),
        ("shaders/weather.vert", "weather.vert.spv"),
        ("shaders/weather.frag", "weather.frag.spv"),
        ("shaders/clouds.frag", "clouds.frag.spv"),
        ("shaders/end_portal.vert", "end_portal.vert.spv"),
        ("shaders/end_portal.frag", "end_portal.frag.spv"),
        ("shaders/end_sky.vert", "end_sky.vert.spv"),
        ("shaders/end_sky.frag", "end_sky.frag.spv"),
        ("shaders/text.vert", "text.vert.spv"),
        ("shaders/velvet_text.vert", "velvet_text.vert.spv"),
        ("shaders/velvet_text.frag", "velvet_text.frag.spv"),
        ("shaders/velvet_chrome.vert", "velvet_chrome.vert.spv"),
        ("shaders/velvet_chrome.frag", "velvet_chrome.frag.spv"),
        ("shaders/text.frag", "text.frag.spv"),
        ("shaders/leash.vert", "leash.vert.spv"),
        ("shaders/leash.frag", "leash.frag.spv"),
        ("shaders/cull.comp", "cull.comp.spv"),
    ] {
        println!("cargo:rerun-if-changed=shaders/lightmap.glsl");
        println!("cargo:rerun-if-changed={src}");
        let output = Command::new(&glslc)
            // `-Ishaders` lets passes share `lightmap.glsl` — the vanilla
            // lightmap formula has to read identically in the world and water
            // passes or translucent blocks light differently from solid ones.
            .args(["--target-env=vulkan1.3", "-O", "-Ishaders", src, "-o"])
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
