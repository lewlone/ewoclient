#version 450
// World-space lines drawn as screen-space quads (the block selection outline),
// `rendertype_lines.vsh`: each vertex knows its own end and the other end of
// its segment, both are projected, and the vertex moves perpendicular to the
// segment's screen direction by half the line width, so the line is
// `pc.screen.z` pixels wide at any distance.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 color;  // rgba
    vec4 screen; // xy: viewport size in pixels, z: line width in pixels
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_other;
layout(location = 2) in float in_side;

void main() {
    vec4 a = pc.view_proj * vec4(in_pos, 1.0);
    vec4 b = pc.view_proj * vec4(in_other, 1.0);
    vec3 na = a.xyz / a.w;
    vec3 nb = b.xyz / b.w;
    vec2 dir = normalize((nb.xy - na.xy) * pc.screen.xy);
    vec2 offset = vec2(-dir.y, dir.x) * pc.screen.z / pc.screen.xy;
    // Both ends see the segment from opposite sides; vanilla makes the offset
    // agree by its x sign.
    if (offset.x < 0.0) {
        offset = -offset;
    }
    gl_Position = vec4((na + vec3(offset * in_side, 0.0)) * a.w, a.w);
}
