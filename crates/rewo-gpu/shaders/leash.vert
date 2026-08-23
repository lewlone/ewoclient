#version 450
// World-space leash ribbon (M170). Positions are absolute world coordinates
// like the terrain and the selection outline; transform by view_proj. The
// per-vertex colour already has the base tint, the alternating-segment dim and
// the interpolated lightmap folded in on the CPU (LINEAR), the way vanilla's
// POSITION_COLOR_LIGHTMAP leash vertex carries `base * colorModifier` and its
// fragment shader multiplies the sampled lightmap — Rewo folds all three so
// the pass needs no lightmap sampler.

layout(push_constant) uniform PC {
    mat4 view_proj;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_color; // linear rgb

layout(location = 0) out vec3 v_color;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_color = in_color;
}
