#version 330 compatibility

/*
    final.fsh — EwoVelvet v0 image pipeline.

    colortex0 (the scene, vanilla-rendered by Iris's built-in gbuffers)
    → linearize → exposure → ACES tonemap → Velvet duotone grade
    → vignette → back to sRGB → interleaved-gradient dither.

    The dither is load-bearing on OLED: the Velvet palette lives in dark
    wine tones where 8-bit banding is most visible.
*/

#include "/lib/settings.glsl"

uniform sampler2D colortex0;
uniform sampler2D colortex1; // blurred bloom (composite → composite1 → composite2)
uniform sampler2D colortex4; // water mask (debug views below)
uniform sampler2D colortex5; // translucency accumulation (debug views below)
uniform sampler2D depthtex0; // triage debug view

in vec2 texcoord;

// Diagnostics — set one to 1, reload, screenshot, set back to 0.
// DEBUG_WATER_MASK: magenta where gbuffers_water wrote the mask.
// DEBUG_WATER_ALPHA: red intensity = accumulated water alpha (coverage).
#define DEBUG_WATER_MASK 0
#define DEBUG_WATER_ALPHA 0
// Triangulation view: cyan = depthtex0 empty (geometry wrote no depth),
// magenta = water mask, yellow = plant flag. One screenshot, three facts.
#define DEBUG_TRIAGE 0

layout(location = 0) out vec4 outColor;

// ── Color space ─────────────────────────────────────────────────────────

vec3 srgbToLinear(vec3 c) {
    return pow(c, vec3(2.2));
}

vec3 linearToSrgb(vec3 c) {
    return pow(c, vec3(1.0 / 2.2));
}

// ── Tonemap (Narkowicz ACES approximation) ──────────────────────────────

vec3 acesFilm(vec3 x) {
    const float a = 2.51;
    const float b = 0.03;
    const float c = 2.43;
    const float d = 0.59;
    const float e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

// ── Velvet grade ────────────────────────────────────────────────────────
// Luma-weighted duotone: shadows drift toward wine, highlights toward
// pearl. Runs in linear light so the tint doesn't crush blacks.

vec3 velvetGrade(vec3 color) {
    float luma = dot(color, vec3(0.2126, 0.7152, 0.0722));
    // Smooth split around mid-grey; t=0 deep shadow, t=1 highlight.
    float t = smoothstep(0.0, 0.85, luma);
    vec3 duotone = mix(
        srgbToLinear(VELVET_SHADOW_TINT) * (luma * 2.2 + 0.02),
        srgbToLinear(VELVET_HIGHLIGHT_TINT) * luma,
        t);
    return mix(color, duotone, VELVET_GRADE);
}

// ── Vignette ────────────────────────────────────────────────────────────
// Matches the launcher backdrop's shape: ellipse biased slightly below
// center (CSS `ellipse at 50% 60%`), gentle falloff.

float vignette(vec2 uv) {
    vec2 d = (uv - vec2(0.5, 0.55)) * vec2(1.15, 1.0);
    float dist = length(d) * 1.4142;
    return 1.0 - VIGNETTE_STRENGTH * smoothstep(0.45, 1.05, dist);
}

// ── Dither (interleaved gradient noise, Jimenez 2014) ───────────────────

float ign(vec2 fragCoord) {
    return fract(52.9829189 * fract(dot(fragCoord, vec2(0.06711056, 0.00583715))));
}

void main() {
    vec3 color = texture(colortex0, texcoord).rgb;

    color = srgbToLinear(color) * EXPOSURE;

#if BLOOM == 1
    // Bloom is already linear (bright-pass converts); add before the
    // tonemap so highlights melt outward filmically.
    color += texture(colortex1, texcoord).rgb * BLOOM_STRENGTH * EXPOSURE;
#endif

#if TONEMAP_ACES == 1
    color = acesFilm(color);
#else
    color = clamp(color, 0.0, 1.0);
#endif

    color = velvetGrade(color);
    color *= vignette(texcoord);

    color = linearToSrgb(color);
    color += (ign(gl_FragCoord.xy) - 0.5) / 255.0;

#if DEBUG_WATER_MASK == 1
    if (texture(colortex4, texcoord).r > 0.5) {
        color = mix(color, vec3(1.0, 0.0, 1.0), 0.6);
    }
#endif
#if DEBUG_TRIAGE == 1
    {
        vec4 m = texture(colortex4, texcoord);
        float d0 = texture(depthtex0, texcoord).r;
        if (d0 >= 1.0) {
            color = mix(color, vec3(0.0, 1.0, 1.0), 0.5); // cyan: no depth
        }
        if (m.r > 0.5) {
            color = mix(color, vec3(1.0, 0.0, 1.0), 0.35); // magenta: water
        }
        if (m.b > 0.5) {
            color = mix(color, vec3(1.0, 1.0, 0.0), 0.5); // yellow: plant flag
        }
    }
#endif
#if DEBUG_WATER_ALPHA == 1
    if (texture(colortex4, texcoord).r > 0.5) {
        color = vec3(texture(colortex5, texcoord).a, 0.0, 0.0);
    }
#endif

    outColor = vec4(color, 1.0);
}
