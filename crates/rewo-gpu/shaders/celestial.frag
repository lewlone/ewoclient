#version 450
// Sun / moon texel × tint. The atlas is an sRGB texture so the sampler returns
// linear; the SRGB attachment re-encodes on store. Blended with vanilla's
// CELESTIAL OVERLAY function (color src=SRC_ALPHA dst=ONE): the disc's alpha
// masks it and its colour adds as a glow over the sky. Fully-transparent texels
// are discarded exactly as vanilla's `position_tex.fsh` does (alpha == 0.0), so
// they never touch the RGBA attachment.

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 tint;
} pc;

layout(set = 0, binding = 0) uniform sampler2D tex;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

#ifdef GAMMA_OUT
// M434 — compiled again for the gamma-space scope (the UNORM view): vanilla's
// `position_tex` multiplies the ENCODED texel by the colour modulator and its
// additive blend adds encoded numbers, so the sun's faint halo texels add tens
// of bytes to the sky where, added in linear light, they all but vanished.
vec3 to_srgb(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}
#endif

void main() {
    vec4 t = texture(tex, v_uv);
#ifdef GAMMA_OUT
    out_color = vec4(to_srgb(t.rgb) * pc.tint.rgb, t.a * pc.tint.a);
#else
    out_color = t * pc.tint;
#endif
    if (out_color.a == 0.0) {
        discard;
    }
}
