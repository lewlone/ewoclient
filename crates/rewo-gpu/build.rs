//! Compile the M0 overlay shaders GLSL -> SPIR-V with glslc (Vulkan SDK).
//! REWO_PLAN.md D4: GLSL + glslc, revisit Slang if/when mesh shaders land.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

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
        ("shaders/line.vert", "line.vert.spv"),
        ("shaders/line.frag", "line.frag.spv"),
        ("shaders/celestial.vert", "celestial.vert.spv"),
        ("shaders/celestial.frag", "celestial.frag.spv"),
        ("shaders/sunrise.vert", "sunrise.vert.spv"),
        ("shaders/sunrise.frag", "sunrise.frag.spv"),
        ("shaders/end_sky.vert", "end_sky.vert.spv"),
        ("shaders/end_sky.frag", "end_sky.frag.spv"),
        ("shaders/text.vert", "text.vert.spv"),
        ("shaders/text.frag", "text.frag.spv"),
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
        // Read back the exact optimized artifact this build will embed. The
        // world pass reconstructs the packed MeshVertex color, and only the
        // emitted SPIR-V can show whether `precise` survived `-O`.
        if dst == "world.vert.spv" {
            verify_world_vert(&out.join(dst));
        }
    }
}

// SPIR-V 1.6 §2.3 (physical layout) and §3 (binary encodings). Only the
// handful of opcodes the color-parity contract turns on are named.
const SPIRV_MAGIC: u32 = 0x0723_0203;
const OP_TYPE_FLOAT: u16 = 22;
const OP_CONSTANT: u16 = 43;
const OP_DECORATE: u16 = 71;
const OP_F_DIV: u16 = 136;
const DECORATION_NO_CONTRACTION: u32 = 42;

/// Assert the world vertex shader still reconstructs the packed MeshVertex
/// color as the CPU mesher's IEEE f32 sequence.
///
/// `world.vert` marks that arithmetic `precise`, but `precise` is a request to
/// the compiler, not a property of the file: at `-O` glslc is free to fold
/// `x / 255.0` into `x * f32(1/255)`, which disagrees by 1 ULP for 126 of the
/// 256 tint bytes. Grepping the GLSL would prove nothing — so this parses the
/// artifact that actually ships and fails the build if the shape is gone. No
/// dependency: SPIR-V is a word stream, and skipping an instruction only needs
/// its leading word.
fn verify_world_vert(path: &Path) {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| fail_parity(&format!("could not read {}: {e}", path.display())));

    // Alignment: a SPIR-V module is a sequence of 32-bit words, nothing else.
    if bytes.len() % 4 != 0 {
        fail_parity(&format!(
            "{} is {} bytes, not a whole number of 32-bit words",
            path.display(),
            bytes.len()
        ));
    }
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    // Header: magic, version, generator, id bound, reserved (§2.3).
    if words.len() < 5 {
        fail_parity(&format!(
            "{} is {} words, too short for the 5-word SPIR-V header",
            path.display(),
            words.len()
        ));
    }
    if words[0] != SPIRV_MAGIC {
        let hint = if words[0].swap_bytes() == SPIRV_MAGIC {
            " (byte-reversed — module is big-endian, this reader assumes little)"
        } else {
            ""
        };
        fail_parity(&format!(
            "{} has magic {:#010x}, expected {SPIRV_MAGIC:#010x}{hint}",
            path.display(),
            words[0]
        ));
    }

    // Result <id>s of every 32-bit OpTypeFloat. Constants are gated on these:
    // an OpConstant carries raw literal words, so the bit pattern of 255.0f32
    // means nothing until its result type is known to be a 32-bit float.
    let mut f32_types: Vec<u32> = Vec::new();
    // (result type <id>, single literal word) for every one-word OpConstant.
    let mut constants: Vec<(u32, u32)> = Vec::new();
    let mut has_fdiv = false;
    let mut has_no_contraction = false;

    let mut i = 5usize;
    while i < words.len() {
        let opcode = (words[i] & 0xFFFF) as u16;
        let count = (words[i] >> 16) as usize;
        // Bounds: a zero word count would spin forever, and a count running
        // past the end means the stream is truncated or misaligned.
        if count == 0 {
            fail_parity(&format!(
                "{}: instruction at word {i} declares a word count of 0",
                path.display()
            ));
        }
        if i + count > words.len() {
            fail_parity(&format!(
                "{}: instruction at word {i} declares {count} words, past the {}-word module",
                path.display(),
                words.len()
            ));
        }
        let inst = &words[i..i + count];
        match opcode {
            // OpTypeFloat: result <id>, width (, optional FP encoding).
            OP_TYPE_FLOAT if count >= 3 && inst[2] == 32 => f32_types.push(inst[1]),
            // OpConstant: result type <id>, result <id>, literal words. A
            // 32-bit float value is exactly one literal word.
            OP_CONSTANT if count == 4 => constants.push((inst[1], inst[3])),
            OP_F_DIV => has_fdiv = true,
            // OpDecorate: target <id>, decoration (, operands).
            OP_DECORATE if count >= 3 && inst[2] == DECORATION_NO_CONTRACTION => {
                has_no_contraction = true
            }
            _ => {}
        }
        i += count;
    }

    let is_f32_const = |bits: u32| {
        constants
            .iter()
            .any(|&(ty, v)| v == bits && f32_types.contains(&ty))
    };

    let mut missing: Vec<&str> = Vec::new();
    if !is_f32_const(255.0f32.to_bits()) {
        missing.push("no 32-bit float OpConstant 255.0 — the divisor is gone");
    }
    if is_f32_const((1.0f32 / 255.0f32).to_bits()) {
        missing.push(
            "a 32-bit float OpConstant f32(1/255) is present — the divide was \
             strength-reduced to a multiply by an inexact reciprocal",
        );
    }
    if !has_fdiv {
        missing.push("no OpFDiv — the tint divide did not survive optimization");
    }
    if !has_no_contraction {
        missing.push("no OpDecorate NoContraction — `precise` was not honored");
    }
    if !missing.is_empty() {
        fail_parity(&format!(
            "{} failed the color-parity shape check:\n  - {}",
            path.display(),
            missing.join("\n  - ")
        ));
    }
}

fn fail_parity(detail: &str) -> ! {
    panic!(
        "world.vert SPIR-V artifact check failed.\n\
         {detail}\n\n\
         This gates packed MeshVertex color parity. The world vertex stage must \
         reconstruct the per-vertex color as `c * (tint_rgb / 255.0)` using the \
         identical IEEE f32 sequence, in the identical order, that the CPU mesher \
         evaluated before the color was dropped from the vertex. Substituting a \
         multiply by f32(1/255) is 1 ULP off for 126 of the 256 tint bytes and \
         shifts real pixels (604 of the canonical 1280x720 `rewo demo` frame).\n\n\
         The GLSL marks that arithmetic `precise`, so a failure here means shader \
         toolchain drift, not a source edit: a different glslc/SPIRV-Tools version \
         or changed optimizer flags stopped honoring it. Compare the glslc in \
         VULKAN_SDK against the one this contract was measured on, and disassemble \
         with `spirv-dis` to see what replaced the OpFDiv."
    )
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
