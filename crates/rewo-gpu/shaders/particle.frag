#version 450
// `assets/minecraft/shaders/core/particle.fsh` (M37):
//
//     vec4 color = texture(Sampler0, texCoord0) * vertexColor * ColorModulator;
//     if (color.a < 0.1) discard;
//     fragColor = apply_fog(...);
//
// `ColorModulator` is white here. The **discard** is load-bearing rather than
// an optimisation: the particle atlas is mostly transparent, and an alpha-blend
// without the cutoff leaves every quad's empty corners as a faint square halo.
//
// Vanilla's fog term is omitted for the same reason every other Rewo pass
// omits it — fog is applied in the world pass, and a second application would
// double it.

// A single array holding the block textures and the particle sprites, so a
// block-break shard and a flame reach the same sampler.
layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in flat uint v_layer;

layout(location = 0) out vec4 out_color;

// M432 — the sRGB transfer, both ways (IEC 61966-2-1). Vanilla shades
// GAMMA-ENCODED numbers into a non-sRGB target; this texture is decoded by
// its sRGB view, so the texel is re-encoded, shaded (and fogged) there, and
// decoded once for the sRGB attachment's own encode.
vec3 to_srgb(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
}
vec3 to_linear(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c));
}

void main() {
    vec4 t = texture(u_tex, vec3(v_uv, float(v_layer)));
    // M432 — `texture * vertexColor` on the gamma-encoded texel, as
    // vanilla's (the vertex colour, its tint times the lightmap, is in that
    // space already).
    vec4 color = vec4(to_srgb(t.rgb) * v_color.rgb, t.a * v_color.a);
    if (color.a < 0.1) {
        discard;
    }
    out_color = vec4(to_linear(color.rgb), color.a);
}
