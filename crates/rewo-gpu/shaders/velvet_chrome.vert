#version 450
// One instanced quad per shell. The quad is the shell rect grown by MARGIN so
// the drop shadow and the music bloom have somewhere to land -- a quad sized
// to the rect alone would clip both, which looks like the shadow "not
// working" rather than like a geometry bug.

layout(push_constant) uniform PC {
    vec2 screen;
} pc;

// Per-instance: rect (x, y, w, h), then (radius, level, pulse, alpha).
layout(location = 0) in vec4 in_rect;
layout(location = 1) in vec4 in_params;

layout(location = 0) out vec2 v_px;
layout(location = 1) out vec4 v_rect;
layout(location = 2) out vec4 v_params;

// Bloom can reach 2 + 6*pulse of growth plus ~1.2 * (10 + 16) of blur support;
// 48 covers the worst case with room to spare.
const float MARGIN = 48.0;

void main() {
    vec2 corner = vec2((gl_VertexIndex & 1), (gl_VertexIndex >> 1) & 1);
    if (gl_VertexIndex >= 3) {
        // second triangle: 3->(1,0) 4->(1,1) 5->(0,1)
        corner = vec2[3](vec2(1, 0), vec2(1, 1), vec2(0, 1))[gl_VertexIndex - 3];
    }
    vec2 lo = in_rect.xy - vec2(MARGIN);
    vec2 hi = in_rect.xy + in_rect.zw + vec2(MARGIN);
    vec2 px = mix(lo, hi, corner);

    v_px = px;
    v_rect = vec4(in_rect.xy + in_rect.zw * 0.5, in_rect.zw * 0.5);
    v_params = in_params;

    gl_Position = vec4(px / pc.screen * 2.0 - 1.0, 0.0, 1.0);
}
