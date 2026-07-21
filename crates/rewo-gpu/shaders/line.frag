#version 450
// Flat line color (the selection outline). Linear rgba from the push,
// alpha-blended; alpha writes masked so readback PNGs stay opaque.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 color;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = pc.color;
}
