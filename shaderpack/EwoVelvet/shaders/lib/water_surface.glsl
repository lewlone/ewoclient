#ifndef WATER_SURFACE_GLSL
#define WATER_SURFACE_GLSL

/*
    EwoWater 2.0 — shared surface material, used by BOTH translucent
    geometry programs (clrwl_gbuffers_translucent for 26.2's colorwheel
    pipeline, gbuffers_water for the legacy route). One source of truth:
    the two programs must be pixel-identical or the pipeline split shows.

    Design (see also lib/water_composite.glsl):
    - The surface itself is OPAQUE and carries only a minimal analytic
      base color; ALL appearance (absorption gradient, mirror, glint,
      foam) is built in composite where every input is controllable.
    - Geometry programs' real job is the G-data contract:
        colortex5: premultiplied surface color (accumulation/OIT target)
        colortex3: world normal (rippled for water) + skylight
        colortex4: waterMask, surface view distance, blocklight, 0
*/

// ── Ripple field ────────────────────────────────────────────────────────
// Three crossed-wave octaves in world space (continuous across chunks),
// returning the surface normal. Amplitude fades with view distance to a
// floor — far water still glints, but per-pixel tilt stops aliasing.

vec3 ewoWaterNormal(vec2 worldXz, float time, float viewDist) {
    float dx = cos(worldXz.x * 0.9 + time * 1.6) * 0.90
             + cos((worldXz.x + worldXz.y) * 0.45 + time * 1.1) * 0.45
             + cos(worldXz.x * 2.3 + time * 2.6) * 0.25
             + cos((worldXz.x * 3.7 - worldXz.y * 1.3) + time * 3.4) * 0.12;
    float dz = cos(worldXz.y * 1.1 + time * 1.9) * 0.90
             + cos((worldXz.x - worldXz.y) * 0.5 + time * 1.3) * 0.45
             + cos(worldXz.y * 2.7 + time * 2.2) * 0.25
             + cos((worldXz.y * 3.1 + worldXz.x * 1.7) + time * 2.9) * 0.12;
    float amp = 0.10 * mix(1.0, 0.18, smoothstep(14.0, 80.0, viewDist));
    return normalize(vec3(-dx * amp, 1.0, -dz * amp));
}

// ── Surface base color ──────────────────────────────────────────────────
// Deliberately minimal: a dark neutral anchor scaled by sky exposure.
// The composite pass repaints water almost entirely (absorption + mirror)
// — this base is what shows if composite is somehow skipped, and what
// non-mirror fractions fall back to. Pure function: cannot vary per
// block, per biome, or per texture frame.

vec4 ewoWaterBase(float skyLight) {
    float exposure = mix(0.25, 1.0, smoothstep(0.1, 0.9, skyLight));
    return vec4(vec3(0.10, 0.19, 0.33) * exposure, 1.0);
}

#endif // WATER_SURFACE_GLSL
