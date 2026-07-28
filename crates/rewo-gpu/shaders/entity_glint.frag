#version 450
// The enchantment glint over world-space geometry — a ground stack or one held
// by a mob (M45), and worn armour (M50). `core/glint.fsh` with the fog term
// dropped, exactly as the GUI and hand glints do.
//
// One shader for both because `ARMOR_ENTITY_GLINT` and `ENTITY_GLINT` differ
// only in their texture and their `TextureTransform` scale — both of which the
// CPU resolves before a vertex reaches here — and in the `VIEW_OFFSET_Z_LAYERING`
// the armour render types carry, which cancels within the armour stack because
// `ARMOR_CUTOUT_NO_CULL` and `ARMOR_DECAL_CUTOUT_NO_CULL` carry it too.
//
// **No lightmap.** Vanilla's glint shader multiplies by `GlintAlpha` and the
// fog fade and nothing else: the sheen is emissive, so a dropped enchanted
// sword shimmers just as brightly in a cave as in daylight. That is why
// `v_light_hurt` is unused here even though the entity vertex carries it.
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
// `a` carries `GlintAlpha`; the rgb are unused.
layout(location = 1) in vec4 v_color;
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_glint, v_uv);
    if (c.a < 0.1) {
        discard;
    }
    out_color = vec4(c.rgb * v_color.a, c.a);
}
