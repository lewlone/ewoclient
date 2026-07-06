#ifndef BLOOM_BLUR_GLSL
#define BLOOM_BLUR_GLSL

/*
    Shared 9-tap separable gaussian for the bloom chain. The caller picks
    the direction; composite1 runs it horizontally (colortex1 → colortex2),
    composite2 vertically (colortex2 → colortex1). viewWidth/viewHeight are
    Iris-provided uniforms declared by the including shader.
*/

vec3 bloomBlur(sampler2D src, vec2 uv, vec2 dir, vec2 pixelSize) {
    const float w[5] = float[](0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
    vec2 stepUv = dir * pixelSize * BLOOM_RADIUS;

    vec3 sum = texture(src, uv).rgb * w[0];
    for (int i = 1; i < 5; i++) {
        sum += texture(src, uv + stepUv * float(i)).rgb * w[i];
        sum += texture(src, uv - stepUv * float(i)).rgb * w[i];
    }
    return sum;
}

#endif // BLOOM_BLUR_GLSL
