#version 450
// Sunrise/sunset fan (18 verts as a triangle fan, expanded to a list on the
// CPU). Per-vertex colour is white with alpha 1 at the centre → alpha 0 at the
// ring; the push tint carries the timeline's SUNRISE_SUNSET_COLOR (linear rgb +
// alpha). MVP = sky view-proj · (X90 · Z(angle+90) · scale z=alpha).

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 tint;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec4 in_color;
layout(location = 0) out vec4 v_color;

void main() {
    v_color = in_color;
    gl_Position = pc.mvp * vec4(in_pos, 1.0);
}
