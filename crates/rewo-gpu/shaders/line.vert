#version 450
// World-space lines (the block selection outline). Positions are already in
// world space; just transform by view_proj.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 color; // rgba, linear
} pc;

layout(location = 0) in vec3 in_pos;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
}
