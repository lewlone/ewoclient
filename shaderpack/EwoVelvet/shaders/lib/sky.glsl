#ifndef SKY_GLSL
#define SKY_GLSL

/*
    The Velvet sky model — one function shared by the skybox
    (gbuffers_skybasic), the atmospheric fog and SSR miss fallback
    (composite1), so all three always agree.

    Built on vanilla's time/biome-aware skyColor + fogColor uniforms
    (declared by the including shader), regraded: zenith deepened toward
    velvet blue, horizon warmed toward pearl, and a rose/ember glow
    around the sun that swells as it drops.
*/

vec3 skyVelvet(vec3 dirWorld, vec3 sunDirWorld, vec3 skyColorU, vec3 fogColorU) {
    float up = clamp(dirWorld.y, -1.0, 1.0);

    // Zenith → horizon gradient from vanilla's own colors, regraded.
    vec3 zenith = pow(skyColorU, vec3(1.12)) * vec3(0.92, 0.92, 1.04);
    vec3 horizon = pow(fogColorU, vec3(0.94)) * vec3(1.05, 0.98, 1.00);
    float h = pow(1.0 - clamp(up, 0.0, 1.0), 2.5);
    vec3 sky = mix(zenith, horizon, h);

    // Sun proximity glow — pearl by day, swelling ember-rose when low.
    float sunDot = clamp(dot(dirWorld, sunDirWorld), 0.0, 1.0);
    float lowSun = 1.0 - clamp(abs(sunDirWorld.y) * 2.4, 0.0, 1.0);
    vec3 glowColor = mix(vec3(1.00, 0.96, 0.90), vec3(1.00, 0.55, 0.45), lowSun * lowSun);
    float glow = pow(sunDot, 10.0) * 0.35 + pow(sunDot, 64.0) * 0.55;
    sky += glowColor * glow * (0.5 + 1.3 * lowSun);

    // Below the horizon: sink into velvet-wine void, not vanilla grey.
    vec3 voidColor = vec3(0.030, 0.012, 0.028);
    sky = mix(sky, voidColor, smoothstep(0.0, -0.35, up));

    return sky;
}

// Direct sun/moon light color: pearl-white overhead, ember-rose near the
// horizon. Shared by the shading pass (sunlight tint) and the water
// specular glint so they always match.
vec3 lightColorVelvet(vec3 lightDirWorld) {
    float horizon = 1.0 - clamp(abs(lightDirWorld.y) * 2.2, 0.0, 1.0);
    vec3 high = vec3(1.0, 0.98, 0.96);
    vec3 low = vec3(1.0, 0.62, 0.52); // ember-rose sunset
    return mix(high, low, horizon * horizon);
}

#endif // SKY_GLSL
