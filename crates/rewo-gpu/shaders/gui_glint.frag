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

void main() {
    vec4 c = texture(u_glint, v_uv);
    if (c.a < 0.1) {
        discard;
    }
    out_color = vec4(c.rgb * v_alpha, c.a);
}
