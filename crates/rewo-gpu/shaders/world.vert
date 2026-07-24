#version 450
// World pass: vertices are already in WORLD space (the mesher emits
// cx*16+lx+corner), so no per-column origin is applied here. The world
// position also rides through to the fragment for distance fog.
//
// M15 packed vertex (28 B): pos f32x3 | uv f32x2 | light u32 | tint u32.
// UV stayed f32: an f16 variant was built and rejected because fluid surface
// UVs (1 - k/9) are not dyadic, and the quantization flipped texel sampling.
// The per-vertex color is NOT stored — it is reconstructed here as
//     c = FACE_SHADE[shade] * AO_LEVELS[ao];  color = c * (tint_rgb / 255)
// which is the identical float sequence, in the identical order, that the
// mesher previously evaluated on the CPU. The product is deliberately NOT
// pre-quantized to RGB8; only the tint bytes are, and those are lossless.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 cam_fog; // xyz camera pos, w = fog start distance
    vec4 fog_col; // xyz fog color (linear), w = fog end distance
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;    // R32G32_SFLOAT (exact)
layout(location = 2) in uint in_light; // layer[0..15] block[16..19] sky[20..23] shade[24..26] ao[27..28]
layout(location = 3) in uint in_tint;  // r[0..7] g[8..15] b[16..23] flags[24..31]

layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_layer;
layout(location = 2) out vec3 v_color;
layout(location = 3) out vec3 v_worldpos;

// Mirrored verbatim from rewo-mesh (`FACE_SHADE` / `AO_LEVELS`) — keep in sync.
const float FACE_SHADE[6] = float[6](1.0, 0.5, 0.8, 0.8, 0.6, 0.6);
const float AO_LEVELS[4] = float[4](0.45, 0.65, 0.82, 1.0);

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;

    // The fragment stage expects the historical lower-24-bit word (layer plus
    // the two light nibbles); the shade/AO codes ride above it and are stripped
    // here so nothing downstream sees the new bits.
    v_layer = in_light & 0x00FFFFFFu;

    // `precise` is load-bearing here, and that is measured rather than assumed.
    // Compiled with this build's own flags (glslc --target-env=vulkan1.3 -O),
    // dropping it makes glslc emit `OpFMul` against a splat of 0.00392156886 —
    // f32(1/255), itself inexact — in place of `OpFDiv` by 255.0. Those two
    // disagree by 1 ULP for 126 of the 256 possible tint bytes. Keeping it, the
    // disassembly holds the `OpFDiv` and adds `NoContraction` to the three
    // arithmetic results, so this stays the IEEE sequence the CPU mesher ran.
    // End to end that ULP is visible: removing `precise` shifts 604 of the
    // 1280×720 canonical `rewo demo` pixels by one channel step (hash
    // 2cc56b4a… → bb5b8024…). This is a claim about the observed behaviour of
    // this compiler at these flags, not a portable language rule.
    precise float c = FACE_SHADE[(in_light >> 24) & 7u] * AO_LEVELS[(in_light >> 27) & 3u];
    vec3 tint_rgb = vec3(
        float(in_tint & 0xFFu),
        float((in_tint >> 8) & 0xFFu),
        float((in_tint >> 16) & 0xFFu)
    );
    precise vec3 color = c * (tint_rgb / 255.0);
    v_color = color;

    v_worldpos = in_pos;
}
