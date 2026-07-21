#version 450
// Sample the HUD sprite atlas (SRGB image → linear; alpha-blended over the
// world). Mask alpha writes so readback PNGs stay opaque.

layout(set = 0, binding = 0) uniform sampler2D u_hud;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_hud, v_uv);
    if (c.a < 0.004) {
        discard;
    }
    out_color = c;
}
