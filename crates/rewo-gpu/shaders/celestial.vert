#version 450
// Sun / moon quads. Positions are in the celestial local frame (a unit quad in
// the XZ plane); the push MVP is the rotation-only sky view-proj times the
// vanilla transform chain (Y-90 · X-angle · translate y=100 · scale). No depth
// — drawn before terrain, which then overwrites it via reversed-Z.

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 tint; // rgba multiply
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 0) out vec2 v_uv;

void main() {
    v_uv = in_uv;
    gl_Position = pc.mvp * vec4(in_pos, 1.0);
}
