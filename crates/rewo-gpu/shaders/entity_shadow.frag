#version 450
// `RenderPipelines.ENTITY_SHADOW` (`core/rendertype_entity_shadow.fsh`): the
// shadow sheet sampled at `clamp(uv, 0, 1)`, times the vertex colour (white
// carrying the piece's alpha), blended TRANSLUCENT through the UNORM view so
// the darkening is vanilla's gamma-space number. The sheet is black with a
// radial alpha, so the colour is zero and the vertex alpha does the work.
//
// The shadow's uv is not confined to the sheet: a piece covers whole blocks
// and the sheet only `[-radius, radius]` around the entity, so the clamp is
// load-bearing (the sheet's border texels are fully transparent, which makes
// everything outside it empty). Vanilla's fog is dropped, as the glint's is,
// and there is no lightmap: `v_light_hurt` is only here to keep the entity
// vertex layout.

layout(set = 0, binding = 0) uniform sampler2D u_shadow;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_shadow, clamp(v_uv, 0.0, 1.0)) * v_color;
    if (c.a < 0.004) {
        discard;
    }
    out_color = c;
}
