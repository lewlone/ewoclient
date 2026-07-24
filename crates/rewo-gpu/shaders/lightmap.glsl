// Vanilla's lightmap, transcribed line-for-line from `assets/minecraft/
// shaders/core/lightmap.fsh` in the 26.2 client jar (the CPU mirror lives in
// `rewo_world::lightmap::sample`).
//
// Block and sky arrive as separate 0..15 levels (packed into the vertex's
// layer word by `rewo_mesh::pack_layer`) and are combined ADDITIVELY here —
// not `max()`. Each goes through the curve below, far darker at low levels
// than a linear ramp, with no floor: an unlit cave is genuinely black. The
// sky half is scaled by `SkyFactor` (time of day), block by `BlockFactor`;
// both come from the lightmap push constants, so a sunset costs a uniform
// update rather than a remesh.

float get_brightness(float level) {
    return level / (4.0 - 3.0 * level);
}

// Vanilla `notGamma`: brightens toward the max component. This has a genuine
// 0/0 singularity at a fully-black texel (maxComponent == 0); it is left
// UNGUARDED to match the source exactly — the RGBA8 lightmap-store behaviour
// is pinned by the M13 Vulkan readback oracle, not papered over here.
vec3 lm_not_gamma(vec3 color) {
    float maxComponent = max(max(color.x, color.y), color.z);
    float maxInverted = 1.0 - maxComponent;
    float maxScaled = 1.0 - maxInverted * maxInverted * maxInverted * maxInverted;
    return color * (maxScaled / maxComponent);
}

// Block light is tinted warm, fading toward white at both ends of the ramp.
float lm_parabolic_mix(float level) {
    return (2.0 * level - 1.0) * (2.0 * level - 1.0);
}

// `packed` is the vertex layer word; `light` = [SkyFactor, BlockFactor,
// BrightnessFactor, DarknessScale] and `sky_col` = [SkyLightColor.rgb,
// NightVisionFactor], exactly as `WorldRenderer::push_world` packs them.
//
// `ambient` is the resolved dimension `AmbientColor`
// (`EnvironmentAttributes.AMBIENT_LIGHT_COLOR`, RGB24/255). The push-constant
// budget is exactly full at 128 bytes, so it arrives through the world pass's
// small `LightmapExtra` uniform buffer (set 0, binding 1) rather than being
// packed lossily into a spare lane. NightVisionColor (0x999999), BlockLightTint
// (0xFFD88C) and the boss-overlay darkening factor (neutral 0) remain the fixed
// attribute-default constants baked in below.
vec3 lm_light(uint packed, vec4 light, vec4 sky_col, vec3 ambient) {
    float block_level = float((packed >> 16) & 15u) / 15.0;
    float sky_level = float((packed >> 20) & 15u) / 15.0;

    float sky_factor = light.x;
    float block_factor = light.y;
    float brightness_factor = light.z;
    float darkness_scale = light.w;
    vec3 sky_color = sky_col.rgb;
    float night_vision_factor = sky_col.w;

    float block_brightness = get_brightness(block_level) * block_factor;
    float sky_brightness = get_brightness(sky_level) * sky_factor;

    // Ambient with or without night vision: max(AmbientColor, nvColor*nvFactor),
    // in the source's argument order. NightVisionColor is 0x999999 (153/255).
    const vec3 NIGHT_VISION_COLOR = vec3(153.0 / 255.0);
    vec3 color = max(ambient, NIGHT_VISION_COLOR * night_vision_factor);

    // Add sky light.
    color += sky_color * sky_brightness;

    // Add block light: tint 0xFFD88C (255/216/140) warmed toward white by
    // 0.9 * parabolic(level).
    const vec3 BLOCK_TINT = vec3(255.0 / 255.0, 216.0 / 255.0, 140.0 / 255.0);
    vec3 block_color = mix(BLOCK_TINT, vec3(1.0), 0.9 * lm_parabolic_mix(block_level));
    color += block_color * block_brightness;

    // Boss-overlay world darkening (pinned neutral: factor 0).
    color = mix(color, color * vec3(0.7, 0.6, 0.6), 0.0);

    // Darkness-effect subtraction, then clamp.
    color = color - vec3(darkness_scale);
    color = clamp(color, 0.0, 1.0);

    // notGamma brightening, blended in by BrightnessFactor. No zero guard.
    vec3 ng = lm_not_gamma(color);
    color = mix(color, ng, brightness_factor);
    return color;
}
