#version 450
// The first-person hand (M38) — the held item and the bare arm, in view space.
//
// Unlike the GUI-item pass this is a real 3D draw: the CPU composes
// `ItemInHandRenderer`'s chain into a model-view and hands it here already
// multiplied by the projection, so the vertex shader only projects.
//
// Positions arrive in **block units** (vertices divided by 16 on the CPU,
// which is where `ModelPart.Cube`'s own 1/16 lives), so the numbers here are
// the same scale as the world's.

layout(push_constant) uniform PC {
    mat4 view_proj;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in float in_shade;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out float v_shade;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_shade = in_shade;
}
