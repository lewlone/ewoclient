#version 450
// Font atlas is white glyphs on alpha; multiply by the per-glyph color so
// the same geometry draws both the black drop shadow and the tinted text.
// The atlas's alpha is the glyph mask.

layout(set = 0, binding = 0) uniform sampler2D u_font;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    float a = texture(u_font, v_uv).a * v_color.a;
    if (a < 0.02) {
        discard;
    }
    out_color = vec4(v_color.rgb, a);
}
