#version 450
// The locator bar (M83): screen-pixel positions (origin top-left), atlas UVs,
// and a per-quad tint. `hud.vert`'s vertex has no colour, and the locator bar
// needs one — every dot carries the waypoint's own colour.

layout(push_constant) uniform PC {
    vec2 screen;
} pc;

layout(location = 0) in vec2 in_pos; // pixels, top-left origin
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color; // gamma-space, see locator.frag

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    vec2 ndc = in_pos / pc.screen * 2.0 - 1.0;
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = in_uv;
    v_color = in_color;
}
