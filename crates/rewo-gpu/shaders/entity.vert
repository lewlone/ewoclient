#version 450
// Entity pass (capsules + nametags): CPU-built world-space triangle soup —
// positions, shading, and billboard orientation are all baked on the CPU
// per frame (entity counts are tiny), so this stays a plain transform.

layout(push_constant) uniform PC {
    mat4 view_proj;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_color = in_color;
}
