// Vanilla's lightmap, transcribed from `assets/minecraft/shaders/core/
// lightmap.fsh` in the 26.2 client jar.
//
// Block and sky arrive as separate 0..15 levels (packed into the vertex's
// layer word by `rewo_mesh::pack_layer`) and are combined ADDITIVELY here —
// not `max()`. Each goes through the same curve, which is far darker at low
// levels than a linear ramp, and has no floor: an unlit cave is genuinely
// black. The sky half is scaled by `sky_factor`, which the time of day drives,
// so a sunset costs a uniform update rather than a remesh.

float lm_brightness(float level) {
    return level / (4.0 - 3.0 * level);
}

// Block light is tinted warm, fading toward white at both ends of the ramp.
float lm_parabolic_mix(float level) {
    return (2.0 * level - 1.0) * (2.0 * level - 1.0);
}

// `light` is the packed layer word; `sky_factor`/`block_factor` and the sky
// colour come from the lightmap push constants.
vec3 lm_light(uint packed, float sky_factor, float block_factor, vec3 sky_color) {
    float block_level = float((packed >> 16) & 15u) / 15.0;
    float sky_level = float((packed >> 20) & 15u) / 15.0;

    float block_brightness = lm_brightness(block_level) * block_factor;
    float sky_brightness = lm_brightness(sky_level) * sky_factor;

    // Overworld ambient is black (AMBIENT_LIGHT_COLOR default 0x000000).
    vec3 color = vec3(0.0);
    color += sky_color * sky_brightness;
    // BLOCK_LIGHT_TINT default 0xFFD86C.
    const vec3 BLOCK_TINT = vec3(1.0, 0.847, 0.424);
    vec3 block_color = mix(BLOCK_TINT, vec3(1.0), 0.9 * lm_parabolic_mix(block_level));
    color += block_color * block_brightness;

    return clamp(color, 0.0, 1.0);
}
