#ifndef SHADOW_DISTORT_GLSL
#define SHADOW_DISTORT_GLSL

/*
    Shadow-map distortion, shared by the shadow pass (shadow.vsh warps
    gl_Position) and every sampler (composite warps the lookup the same
    way). Radial warp concentrates the fixed-size map's resolution near
    the camera — the standard Minecraft shadow trick. The two sides MUST
    stay in sync or shadows shear apart.
*/

const float SHADOW_DISTORT_FACTOR = 0.10;

float shadowDistortionFactor(vec2 clipXy) {
    return length(clipXy) * (1.0 - SHADOW_DISTORT_FACTOR) + SHADOW_DISTORT_FACTOR;
}

vec3 distortShadow(vec3 clipPos) {
    float factor = shadowDistortionFactor(clipPos.xy);
    return vec3(clipPos.xy / factor, clipPos.z * 0.5);
}

#endif // SHADOW_DISTORT_GLSL
