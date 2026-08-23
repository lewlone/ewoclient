#version 450
// Flat per-vertex leash colour (M170). Linear rgb straight from the vertex;
// alpha 1 with alpha writes masked, so readback PNGs stay opaque — the same
// discipline as line.frag.

layout(location = 0) in vec3 v_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(v_color, 1.0);
}
