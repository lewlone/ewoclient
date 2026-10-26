#version 450
// M286 — `RenderPipelines.BREEZE_WIND` (`core/entity` with ALPHA_CUTOUT 0.1,
// APPLY_TEXTURE_MATRIX, NO_OVERLAY, NO_CARDINAL_LIGHTING): the breeze's wind
// funnel. The texture transform's offset is already in the UVs (the sheet's
// sampler repeats), the vertex colour is white, and the lightmap multiplies
// the texel. Blended TRANSLUCENT through the UNORM view, so the texel, the
// light and the blend are vanilla's gamma-space numbers: the light arrives
// linear (the entity vertex's ABI) and is encoded back here. M412 —
// `apply_fog` toward the gamma-encoded fog colour.

layout(set = 0, binding = 0) uniform sampler2D u_wind;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

// M412 — vanilla's fog (`fog.glsl`), as `world.frag` applies it: the
// render-distance band over the cylindrical distance, the environmental band
// over the spherical one, the larger of the two. In the fragment-only push
// range after the vertex stage's `view_proj`.
layout(push_constant) uniform Fog {
    layout(offset = 64) vec4 cam_fog; // xyz camera, w render-distance fog start
    vec4 fog_col;                     // rgb fog colour (linear), w its end
    vec4 env_fog;                     // x environmental start, y its end
} fog;

layout(location = 3) in vec3 v_worldpos;

// M421 — `fog.glsl`'s `linear_fog_value`: piecewise, with no floor on the
// band's width (the 1-block floor Rewo divided by halved lava's 0.25..1 band;
// an empty band, start == end, is a step).
float linear_fog_value(float d, float s, float e) {
    if (d <= s) {
        return 0.0;
    } else if (d >= e) {
        return 1.0;
    }
    return (d - s) / (e - s);
}

float fog_render() {
    vec3 rel = v_worldpos - fog.cam_fog.xyz;
    float cyl = max(length(rel.xz), abs(rel.y));
    return linear_fog_value(cyl, fog.cam_fog.w, fog.fog_col.w);
}

float fog_env() {
    float dist = length(v_worldpos - fog.cam_fog.xyz);
    return linear_fog_value(dist, fog.env_fog.x, fog.env_fog.y);
}

float fog_amount() {
    return max(fog_render(), fog_env());
}

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
    vec3 lit = c.rgb * to_srgb(v_light_hurt.rgb);
    out_color = vec4(mix(lit, to_srgb(fog.fog_col.rgb), fog_amount()), c.a);
}
