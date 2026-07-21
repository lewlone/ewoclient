#version 450
// M2 world pass: chunk-local positions + per-draw column origin.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 origin; // column origin in world space (w unused)
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in uint in_layer;
layout(location = 3) in float in_shade;

layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_layer;
layout(location = 2) out float v_shade;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos + pc.origin.xyz, 1.0);
    v_uv = in_uv;
    v_layer = in_layer;
    v_shade = in_shade;
}
