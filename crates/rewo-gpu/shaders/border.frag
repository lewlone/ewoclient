#version 450
// `core/rendertype_world_border.fsh`, verbatim:
//
//     vec4 color = texture(Sampler0, texCoord0);
//     if (color.a == 0.0) { discard; }
//     fragColor = color * ColorModulator;
//
// The `== 0.0` is exact, not a threshold — `forcefield.png`'s transparent
// regions are fully transparent, and a partly-transparent texel still draws
// (its alpha then feeds the OVERLAY blend's `SRC_ALPHA` weight).
//
// The colour arrives **already linearised** on the CPU. `BlendFunction.OVERLAY`
// is `(SRC_ALPHA, ONE)`, an additive blend, and the attachment re-encodes to
// sRGB on store — so a gamma-space addend would come out disproportionate, the
// same reason `end_sky` and `clouds` linearise their constants.

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 color;
    vec2 tex_offset;
    vec2 _pad;
} pc;

layout(set = 0, binding = 0) uniform sampler2D u_tex;

layout(location = 0) in vec2 v_uv;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(u_tex, v_uv);
    if (color.a == 0.0) {
        discard;
    }
    out_color = color * pc.color;
}
