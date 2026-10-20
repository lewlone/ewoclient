#version 450
// M286 — `RenderPipelines.BREEZE_WIND` (`core/entity` with ALPHA_CUTOUT 0.1,
// APPLY_TEXTURE_MATRIX, NO_OVERLAY, NO_CARDINAL_LIGHTING): the breeze's wind
// funnel. The texture transform's offset is already in the UVs (the sheet's
// sampler repeats), the vertex colour is white, and the lightmap multiplies
// the texel. Blended TRANSLUCENT through the UNORM view, so the texel, the
// light and the blend are vanilla's gamma-space numbers: the light arrives
// linear (the entity vertex's ABI) and is encoded back here. Fog is dropped,
// as the glint's is.

layout(set = 0, binding = 0) uniform sampler2D u_wind;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

vec3 to_srgb(vec3 c) {
    return mix(c * 12.92,
               1.055 * pow(max(c, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055,
               greaterThan(c, vec3(0.0031308)));
}

void main() {
    vec4 c = texture(u_wind, v_uv) * v_color;
    if (c.a < 0.1) {
        discard;
    }
    out_color = vec4(c.rgb * to_srgb(v_light_hurt.rgb), c.a);
}
