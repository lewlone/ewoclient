#version 450
// Sample the HUD sprite atlas (SRGB image → linear; alpha-blended over the
// world), multiplied by the vertex tint. Mask alpha writes so readback PNGs
// stay opaque.
//
// **The tint is applied in LINEAR space**, because the atlas is an SRGB image
// and `texture()` has already decoded. Every blit that predates M109 passes
// `[1, 1, 1, 1]`, an exact no-op. The one tinted caller — the chat backdrop —
// asks for black at a varying alpha, and black is 0 in both spaces, so its
// colour needs no transfer-function care either. A caller wanting a *mid*
// tone would: it must hand over a linear value, not the sRGB byte.

layout(set = 0, binding = 0) uniform sampler2D u_hud;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_hud, v_uv) * v_color;
    if (c.a < 0.004) {
        discard;
    }
    out_color = c;
}
