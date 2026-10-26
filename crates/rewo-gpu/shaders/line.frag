#version 450
// Flat line colour (the selection outline), alpha-blended; alpha writes masked
// so readback PNGs stay opaque.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 color;
    vec4 screen;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = pc.color;
}
