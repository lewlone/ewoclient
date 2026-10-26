#version 450
// The enchantment glint over a GUI item (M43) — `core/glint.fsh`, minus the
// fog term a screen-space pass has no use for:
//
//   vec4 color = texture(Sampler0, texCoord0) * ColorModulator;
//   if (color.a < 0.1) discard;
//   float fade = (1 - fog) * GlintAlpha;
//   fragColor = vec4(color.rgb * fade, color.a);
//
// `ColorModulator` is white here, and `fade` collapses to `GlintAlpha`, which
// arrives per-vertex in the slot the item pass uses for its diffuse shade.
//
// The blend is what makes it a sheen rather than a wash: `SRC_COLOR, ONE` on
// colour, so the glint's own brightness scales its contribution and it only
// ever *adds*. A dark texel adds nothing; a bright one blooms.
//
// M50 — **this runs in gamma space**, and that is load-bearing rather than
// incidental. `BlendFunction.GLINT` is `(SRC_COLOR, ONE)`, so the contribution
// is `src²`, and squaring is not invariant under the sRGB transfer function:
// blended into an sRGB attachment the same expression loses almost all of the
// sheen (a worn armour foil measured a byte-delta of exactly 0). Vanilla binds
// no sRGB framebuffer and no sRGB texture view, so both the sampled texel and
// the destination are gamma-encoded numbers — and Rewo reproduces that by
// rendering the glint through a UNORM view of the same image and uploading the
// sheet UNORM. So the line below is vanilla's, verbatim, and needs no
// conversion in either direction.

layout(set = 0, binding = 0) uniform sampler2D u_glint;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_alpha;

layout(location = 0) out vec4 out_color;

#ifdef HAND_FOG
// M430 — compiled again as the first-person hand's glint: vanilla's `fade =
// (1 - fog) · GlintAlpha` with the world's environmental fog, which the hand
// inherits (M423; the render-distance band never reaches a hand).
layout(push_constant) uniform Fog {
    layout(offset = 64) vec4 fog_col; // unused here
    vec4 env_fog;                     // x environmental start, y its end
} fog;

layout(location = 2) in float v_dist;

// `fog.glsl`'s `linear_fog_value` (M421).
float linear_fog_value(float d, float s, float e) {
    if (d <= s) {
        return 0.0;
    } else if (d >= e) {
        return 1.0;
    }
    return (d - s) / (e - s);
}
#endif

void main() {
    vec4 c = texture(u_glint, v_uv);
    if (c.a < 0.1) {
        discard;
    }
    float fade = v_alpha;
#ifdef HAND_FOG
    fade *= 1.0 - linear_fog_value(v_dist, fog.env_fog.x, fog.env_fog.y);
#endif
    out_color = vec4(c.rgb * fade, c.a);
}
