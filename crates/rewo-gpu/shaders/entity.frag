#version 450
// Sample the font atlas (SRGB image → linear texels). Solid geometry
// (capsules, tag backgrounds) points its UVs at the atlas's patched white
// texel, so one pipeline family covers glyphs and fills. Vertex colors
// arrive pre-linearized from the CPU (render discipline: sRGB constants
// never hit an SRGB attachment raw).

layout(set = 0, binding = 0) uniform sampler2D u_font;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 t = texture(u_font, v_uv);
    float a = t.a * v_color.a;
    if (a < 0.004) {
        discard;
    }
    out_color = vec4(t.rgb * v_color.rgb, a);
}
