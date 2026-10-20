#version 450
// M278 — vanilla's `rendertype_outline.fsh`: a glowing entity's geometry in
// its outline colour, wherever its texture is not fully transparent:
//
//     vec4 color = texture(Sampler0, texCoord0);
//     if (color.a == 0.0) discard;
//     fragColor = vec4(ColorModulator.rgb * vertexColor.rgb, ColorModulator.a);
//
// The colour is the team's, in gamma-encoded numbers: the outline target is
// UNORM, as vanilla's RGBA8 target is, and so is the view the composite blends
// into.

layout(set = 0, binding = 0) uniform sampler2D u_font;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 t = texture(u_font, v_uv);
    if (t.a == 0.0) {
        discard;
    }
    out_color = vec4(v_color.rgb, 1.0);
}
