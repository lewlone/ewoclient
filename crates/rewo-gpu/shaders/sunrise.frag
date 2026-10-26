#version 450
// Sunrise/sunset colour = per-vertex colour × tint. Vanilla blends this with
// TRANSLUCENT (src=SRC_ALPHA dst=ONE_MINUS_SRC_ALPHA): the alpha-1 centre
// paints the warm colour, fading to nothing at the alpha-0 ring. Fully-
// transparent fragments are discarded exactly as vanilla's `position_color.fsh`
// does (alpha == 0.0).

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 tint;
} pc;

layout(location = 0) in vec4 v_color;
layout(location = 0) out vec4 out_color;

#ifdef GAMMA_OUT
// M434 — the gamma-space variant: the tint arrives linear and is encoded, so
// the fan's translucent blend runs on encoded numbers as vanilla's does.
vec3 to_srgb(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}
#endif

void main() {
    out_color = v_color * pc.tint;
#ifdef GAMMA_OUT
    out_color.rgb = to_srgb(out_color.rgb);
#endif
    if (out_color.a == 0.0) {
        discard;
    }
}
