#version 450
// World-space lines (`celestial.rs`; the selection outline has its own,
// `selection_line.vert`). Positions are in world space; transform by view_proj.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 color; // rgba, linear
} pc;

layout(location = 0) in vec3 in_pos;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
}
