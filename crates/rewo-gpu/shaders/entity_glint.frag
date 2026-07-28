#version 450
// The enchantment glint over a world-space item (M45) — a ground stack or one
// held by a mob. `core/glint.fsh` with the fog term dropped, exactly as the
// GUI and hand glints do.
//
// **No lightmap.** Vanilla's glint shader multiplies by `GlintAlpha` and the
// fog fade and nothing else: the sheen is emissive, so a dropped enchanted
// sword shimmers just as brightly in a cave as in daylight. That is why
// `v_light_hurt` is unused here even though the entity vertex carries it.

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
