#version 450
// Sample the font atlas (SRGB image → linear texels). Solid geometry
// (capsules, tag backgrounds) points its UVs at the atlas's patched white
// texel, so one pipeline family covers glyphs and fills. Vertex colors
// arrive pre-linearized from the CPU (render discipline: sRGB constants
// never hit an SRGB attachment raw).
//
// M21 — the damage flash. Vanilla's `entity.fsh` is:
//
//     color *= faceVertexColor * ColorModulator;
//     color.rgb = mix(overlayColor.rgb, color.rgb, overlayColor.a);
//     color *= lightMapColor;
//
// so the overlay lands on `texture * vertexColor` and the lightmap multiplies
// *after* it — which is why `v_light_hurt.rgb` carries the light separately
// instead of the CPU folding it into `v_color`. `overlayColor` is a texel of
// `OverlayTexture`: the red row (v=3) is 0xB2FF0000 (`-1291911168` in
// 26.2), i.e. rgb (1,0,0) with a = 178/255, and the no-overlay texel (u=0,
// v=10) is white with a = 1.0 — which makes the mix a no-op, exactly as
// vanilla intends. M277 — the white rows (v >= 8) at column u are white with
// a = floor((1 - u/15 · 0.75) · 255) / 255: the creeper's and lit TNT's
// flash. The CPU packs the overlay into `v_light_hurt.a`: 1 is the red row
// (which wins whatever u is, as every red texel is red), 2 + u/15 a white
// column, 0 none.

layout(set = 0, binding = 0) uniform sampler2D u_font;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
// rgb = per-channel world light, a = the overlay (see above).
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

// `OverlayTexture`'s red row alpha, 0xB2 = 178.
const float HURT_OVERLAY_ALPHA = 178.0 / 255.0;

// Vanilla shades in its gamma-encoded texture space; `mix` and the multiply
// are not invariant under the transfer function, so the chain runs encoded
// (M433) and is decoded once.
vec3 to_srgb(vec3 c) {
    c = clamp(c, 0.0, 1.0);
    return mix(c * 12.92,
               1.055 * pow(max(c, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055,
               greaterThan(c, vec3(0.0031308)));
}

vec3 to_linear(vec3 c) {
    return mix(c / 12.92,
               pow((c + 0.055) / 1.055, vec3(2.4)),
               greaterThan(c, vec3(0.04045)));
}

// M306 — compiled a second time with GAMMA_OUT for `ENTITY_TRANSLUCENT`:
// vanilla's `ALPHA_CUTOUT` 0.1, and the colour leaves sRGB-encoded because
// that pass blends through the UNORM view (vanilla blends gamma-encoded
// values; the solid pass's attachment encodes the same colour on store).
#ifdef GAMMA_OUT
const float ALPHA_MIN = 0.1;
#else
const float ALPHA_MIN = 0.004;
#endif

void main() {
    vec4 t = texture(u_font, v_uv);
    float a = t.a * v_color.a;
    if (a < ALPHA_MIN) {
        discard;
    }
    // M433 — the whole chain on gamma-encoded numbers, as vanilla's: the
    // texel and the vertex colour (handed over linear, `gamma_vertex`) are
    // re-encoded and multiplied, the overlay mixed, the light (a lightmap
    // sample, already vanilla's number) multiplied and the fog mixed there,
    // and the result decoded once for the sRGB attachment. Until M433 the
    // texel was multiplied linear, so the light and the face shade each acted
    // as f^(1/2.2) (mobs too bright, as terrain was before M432).
    vec3 s = to_srgb(t.rgb) * to_srgb(v_color.rgb);
    if (v_light_hurt.a > 0.5 && v_light_hurt.a < 1.5) {
        // mix(overlay.rgb, color.rgb, overlay.a) against the red row's texel.
        s = mix(vec3(1.0, 0.0, 0.0), s, HURT_OVERLAY_ALPHA);
    } else if (v_light_hurt.a >= 1.5) {
        // M277 — a white column's texel.
        float u = floor((v_light_hurt.a - 2.0) * 15.0 + 0.5);
        float wa = floor((1.0 - u / 15.0 * 0.75) * 255.0) / 255.0;
        s = mix(vec3(1.0), s, wa);
    }
    s *= v_light_hurt.rgb;
    // M412 — `apply_fog`, last, toward the fog colour (alpha kept).
    s = mix(s, to_srgb(fog.fog_col.rgb), fog_amount());
#ifdef GAMMA_OUT
    out_color = vec4(s, a);
#else
    out_color = vec4(to_linear(s), a);
#endif
}
