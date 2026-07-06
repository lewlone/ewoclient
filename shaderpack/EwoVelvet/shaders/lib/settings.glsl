#ifndef SETTINGS_GLSL
#define SETTINGS_GLSL

/*
    EwoVelvet settings — every user-tunable knob lives here.

    Iris parses `#define NAME value // [allowed values]` into the shader
    pack settings UI; entries listed in shaders.properties `sliders=`
    render as sliders. Values must appear in the allowed list.
*/

// Scene exposure applied in linear light, before tonemapping.
#define EXPOSURE 1.00 // [0.60 0.70 0.80 0.90 1.00 1.10 1.20 1.30 1.40 1.60 1.80 2.00]

// Filmic tonemap (ACES approximation). Off = clamp only.
#define TONEMAP_ACES 1 // [0 1]

// Strength of the Velvet duotone grade: shadows lean wine, highlights
// lean pearl. 0 disables. Keep subtle — gameplay readability wins.
#define VELVET_GRADE 0.25 // [0.00 0.05 0.10 0.15 0.20 0.25 0.30 0.40 0.50 0.65 0.80 1.00]

// Corner darkening. 0 disables.
#define VIGNETTE_STRENGTH 0.20 // [0.00 0.05 0.10 0.15 0.20 0.25 0.30 0.40 0.50]

// ── Bloom ───────────────────────────────────────────────────────────────

// HDR glow around bright pixels (torches, sun, glowstone). Off skips all
// three bloom passes' sampling work.
#define BLOOM 1 // [0 1]

// How much bloom is added back into the scene (linear, pre-tonemap).
#define BLOOM_STRENGTH 0.35 // [0.10 0.15 0.20 0.25 0.30 0.35 0.40 0.50 0.65 0.80 1.00]

// Luma above which a pixel starts contributing (soft knee below it).
#define BLOOM_THRESHOLD 0.72 // [0.50 0.55 0.60 0.65 0.72 0.80 0.88 0.95]

// Gaussian tap spacing in texels — larger = wider, softer halo.
#define BLOOM_RADIUS 3.0 // [1.0 2.0 3.0 4.0 5.0 6.0]

// Pearl warmth pulled into the glow color (identity: highlights read
// pearl, not clinical white). 0 = untinted.
const float BLOOM_PEARL_TINT = 0.22;

// ── Shadows ─────────────────────────────────────────────────────────────

// Sun/moon shadows from the shadow map. Off skips all shadow sampling.
#define SHADOWS 1 // [0 1]

// PCF kernel spread in shadow-map texels — higher = softer edges.
#define SHADOW_SOFTNESS 1.0 // [0.5 1.0 1.5 2.0 3.0]

// How dark full shadow gets (multiplier floor). Shadowed light leans
// wine, not neutral grey — the identity ambient.
#define SHADOW_DARKNESS 0.42 // [0.25 0.30 0.35 0.42 0.50 0.60 0.70]

// Wine ambience mixed into shadowed light (sRGB, linearized in use).
const vec3 SHADOW_AMBIENT_TINT = vec3(0.86, 0.78, 0.88);

// ── Volumetric light ────────────────────────────────────────────────────

// Sun/moon shafts raymarched through the shadow map.
#define VOLUMETRIC_LIGHT 1 // [0 1]

// Shaft intensity.
#define VL_STRENGTH 0.40 // [0.10 0.20 0.30 0.40 0.55 0.70 0.90 1.20]

// Raymarch steps (cost scales linearly).
#define VL_STEPS 12 // [8 12 16 24]

// ── Water ───────────────────────────────────────────────────────────────

// Vertex bob + animated ripple normals + Velvet-teal tint on water.
#define WATER_WAVES 1 // [0 1]

// Pearl foam line where water meets land (depth-difference based).
#define WATER_FOAM 1 // [0 1]

// Screen-space reflections on the water surface (sky fallback on miss).
#define WATER_SSR 1 // [0 1]

// Reflection intensity (scales the fresnel blend).
#define SSR_STRENGTH 1.0 // [0.4 0.6 0.8 1.0 1.2 1.5]

// ── Sky & fog ───────────────────────────────────────────────────────────

// The Velvet sky: regraded zenith/horizon gradient, sun-proximity glow
// (ember-rose when low), wine void below the horizon, pearl stars.
#define VELVET_SKY 1 // [0 1]

// Distant terrain dissolves toward the sky model instead of vanilla's
// hard fog wall.
#define VELVET_FOG 1 // [0 1]

// Where fog reaches half strength, as a fraction of render distance.
#define FOG_DISTANCE 0.65 // [0.35 0.45 0.55 0.65 0.80 1.00 1.30]

// ── Non-UI constants (identity palette, from ewo-core::theme::VELVET) ──

// Wine shadow tint (#120010 hue family, lifted so the lerp stays gentle).
const vec3 VELVET_SHADOW_TINT = vec3(0.145, 0.075, 0.135);
// Pearl highlight tint (#F4E8EA).
const vec3 VELVET_HIGHLIGHT_TINT = vec3(0.957, 0.910, 0.918);

#endif // SETTINGS_GLSL
