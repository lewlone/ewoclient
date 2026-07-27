#version 450
// The container screen's panel, backdrop and slot highlights (M35).
//
// Screen pixels, top-left origin, like the HUD and GUI-item passes. Colour is
// per-vertex so one pipeline draws both the textured blits and the untextured
// backdrop gradient — the latter samples a white texel, so the multiply in the
// fragment shader leaves the vertex colour alone.

layout(push_constant) uniform PC {
    vec2 screen;
} pc;

layout(location = 0) in vec2 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;   // sRGB, straight alpha

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    gl_Position = vec4(in_pos / pc.screen * 2.0 - 1.0, 0.0, 1.0);
    v_uv = in_uv;
    v_color = in_color;
}
