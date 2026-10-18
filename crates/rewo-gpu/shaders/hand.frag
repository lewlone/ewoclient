#version 450
// Texture times the per-face shade. The alpha cutout is the item path's:
// a sprite item is mostly transparent, and blending its edge would fringe
// against the world behind it.
//
// The arm's sleeve is genuinely translucent in vanilla
// (`RenderTypes.entityTranslucent`), but at the cutout threshold below the
// difference is a fringe of at most one texel on a 4-px-wide box, so the
// simpler cutout is used for both and the sleeve reads correctly.

layout(set = 0, binding = 0) uniform sampler2D u_atlas;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_shade;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_atlas, v_uv);
    // M213: a NEGATIVE shade marks a translucent quad — a held map's banner
    // label background, `Font`'s 0x80000000 effect rect — which blends with
    // its texel's alpha instead of taking the cutout.
    if (v_shade < 0.0) {
        if (c.a <= 0.0) {
            discard;
        }
        out_color = vec4(c.rgb * -v_shade, c.a);
        return;
    }
    if (c.a < 0.1) {
        discard;
    }
    out_color = vec4(c.rgb * v_shade, 1.0);
}
