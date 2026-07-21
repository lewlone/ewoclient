#version 450
// World pass: vertices are already in WORLD space (the mesher emits
// cx*16+lx+corner), so no per-column origin is applied here.

layout(push_constant) uniform PC {
    mat4 view_proj;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in uint in_layer;
layout(location = 3) in vec3 in_color;

layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_layer;
layout(location = 2) out vec3 v_color;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_layer = in_layer;
    v_color = in_color;
}
