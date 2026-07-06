#version 330 compatibility

/*
    composite.fsh — deferred sun shading + volumetric light. Reads the
    vanilla-lit scene (colortex0), the gbuffer data block (colortex3:
    world normal + skylight) and the shadow map; writes the shaded scene
    back to colortex0. The bloom chain (composite1..3) runs after.

    Shading model: "shadow the vanilla image", not full relighting —
    sun-reached surfaces keep their vanilla color; shadowed ones multiply
    toward a wine-tinted ambient floor (SHADOW_DARKNESS). Skylight from
    the lightmap masks caves and roofed areas out; surfaces that never
    wrote RT3 (entities, hand) have skylight 0 and pass through untouched.

    Shadow-map consts live here (any .fsh works; this pass owns shadows):
*/

const int shadowMapResolution = 2048; // [1024 2048 4096]
const float shadowDistance = 160.0;
const float sunPathRotation = -30.0;

#include "/lib/settings.glsl"
#include "/lib/shadow_distort.glsl"

uniform sampler2D colortex0;
uniform sampler2D colortex3;
uniform sampler2D colortex4;
uniform sampler2D depthtex0;
uniform sampler2D shadowtex1;

uniform mat4 gbufferProjectionInverse;
uniform mat4 gbufferModelViewInverse;
uniform mat4 shadowModelView;
uniform mat4 shadowProjection;

uniform vec3 shadowLightPosition;
uniform float sunAngle;
uniform float rainStrength;
uniform float viewWidth;
uniform float viewHeight;

in vec2 texcoord;

/* RENDERTARGETS: 0 */
layout(location = 0) out vec4 outColor;

// Interleaved gradient noise — jitters PCF rotation + VL march start.
float ign(vec2 fragCoord) {
    return fract(52.9829189 * fract(dot(fragCoord, vec2(0.06711056, 0.00583715))));
}

// Player-relative world position of this fragment.
vec3 worldPosFromDepth(float depth) {
    vec4 clip = vec4(vec3(texcoord, depth) * 2.0 - 1.0, 1.0);
    vec4 view = gbufferProjectionInverse * clip;
    view /= view.w;
    return (gbufferModelViewInverse * view).xyz;
}

// World position → distorted shadow-map screen coords (xy: 0..1, z: cmp).
vec3 toShadowScreen(vec3 worldPos) {
    vec4 shadowClip = shadowProjection * (shadowModelView * vec4(worldPos, 1.0));
    shadowClip.xyz = distortShadow(shadowClip.xyz);
    return shadowClip.xyz * 0.5 + 0.5;
}

float sampleShadow(vec3 shadowScreen, float bias) {
    if (clamp(shadowScreen.xy, 0.0, 1.0) != shadowScreen.xy) {
        return 1.0; // outside the map → assume lit
    }
    return step(shadowScreen.z - bias, texture(shadowtex1, shadowScreen.xy).r);
}

// 8-tap rotated-disk PCF.
float softShadow(vec3 worldPos, vec3 worldNormal, vec3 sunDirWorld) {
    // Normal + light-direction offset kills most acne before biasing.
    vec3 biased = worldPos + worldNormal * 0.06 + sunDirWorld * 0.02;
    vec3 center = toShadowScreen(biased);

    float angle = ign(gl_FragCoord.xy) * 6.2831853;
    vec2 rot = vec2(cos(angle), sin(angle));
    float spread = SHADOW_SOFTNESS / float(shadowMapResolution);
    float bias = 0.0006;

    float sum = sampleShadow(center, bias);
    const vec2 taps[7] = vec2[](
        vec2( 1.0,  0.0), vec2(-0.7,  0.7), vec2( 0.0, -1.0),
        vec2( 0.7,  0.7), vec2(-1.0,  0.0), vec2( 0.4, -0.6),
        vec2(-0.4, -0.3));
    for (int i = 0; i < 7; i++) {
        vec2 o = vec2(taps[i].x * rot.x - taps[i].y * rot.y,
                      taps[i].x * rot.y + taps[i].y * rot.x) * spread * float(i + 1) * 0.5;
        sum += sampleShadow(center + vec3(o, 0.0), bias);
    }
    return sum / 8.0;
}

// Sun/moon color: pearl-white overhead, ember-rose near the horizon.
vec3 lightColor(vec3 sunDirWorld) {
    float horizon = 1.0 - clamp(abs(sunDirWorld.y) * 2.2, 0.0, 1.0);
    vec3 high = vec3(1.0, 0.98, 0.96);
    vec3 low = vec3(1.0, 0.62, 0.52); // ember-rose sunset
    return mix(high, low, horizon * horizon);
}

void main() {
    vec3 color = texture(colortex0, texcoord).rgb;
    float depth = texture(depthtex0, texcoord).r;
    vec4 data = texture(colortex3, texcoord);

    vec3 sunDirWorld = normalize(mat3(gbufferModelViewInverse) * shadowLightPosition);
    float wet = 1.0 - rainStrength;

#if SHADOWS == 1
    // Water is excluded: terrain-style sun shading on a rippled mirror
    // reads as patchy dark bands, and the shadow system's distance fade
    // draws visible seams across open ocean. Water's light comes from
    // the reflection layer (composite1) instead — uniform by construction.
    if (depth < 1.0 && data.a > 0.01 && texture(colortex4, texcoord).r < 0.5) {
        vec3 worldPos = worldPosFromDepth(depth);
        float dist = length(worldPos);
        // Fade shadows out at the map edge instead of hard-clipping.
        float distFade = smoothstep(shadowDistance * 0.9, shadowDistance * 0.6, dist);

        if (distFade > 0.001) {
            vec3 normal = data.rgb * 2.0 - 1.0;
            float skylight = smoothstep(0.55, 0.95, data.a);
            float ndotl = clamp(dot(normal, sunDirWorld), 0.0, 1.0);

            float lit = softShadow(worldPos, normal, sunDirWorld) * ndotl;
            // How much this pixel *could* be sunlit → how far it may drop.
            float exposure = skylight * distFade * wet;

            vec3 shadowFloor = SHADOW_DARKNESS * pow(SHADOW_AMBIENT_TINT, vec3(2.2));
            vec3 linScene = pow(color, vec3(2.2));
            vec3 shaded = mix(shadowFloor * linScene, linScene * lightColor(sunDirWorld), lit);
            color = pow(mix(linScene, shaded, exposure), vec3(1.0 / 2.2));
        }
    }
#endif

#if VOLUMETRIC_LIGHT == 1
    {
        vec3 worldPos = worldPosFromDepth(depth);
        float rayLen = min(length(worldPos), shadowDistance * 0.6);
        vec3 rayDir = normalize(worldPos);

        // Forward-scattering phase: shafts bloom looking toward the light.
        float vdotl = clamp(dot(rayDir, sunDirWorld), 0.0, 1.0);
        float phase = 0.15 + 0.85 * pow(vdotl, 6.0);

        float jitter = ign(gl_FragCoord.xy);
        float stepLen = rayLen / float(VL_STEPS);
        float visible = 0.0;
        for (int i = 0; i < VL_STEPS; i++) {
            vec3 p = rayDir * stepLen * (float(i) + jitter);
            visible += sampleShadow(toShadowScreen(p), 0.0008);
        }
        visible /= float(VL_STEPS);

        // Damp shafts at night: moonlight is subtle, and the raymarch's
        // per-pixel jitter grain reads as surface noise on dark water.
        // NOTE sunDirWorld tracks the shadow light (= the MOON at night),
        // so day/night comes from sunAngle: sin(2π·angle) is the actual
        // sun height (1 at noon, negative all night).
        float sunUp = clamp(sin(sunAngle * 6.2831853), 0.0, 1.0);
        float dayLift = mix(0.30, 1.0, smoothstep(0.0, 0.25, sunUp));
        vec3 shaft = lightColor(sunDirWorld) * visible * phase
                   * VL_STRENGTH * wet * 0.22 * dayLift;
        color = pow(pow(color, vec3(2.2)) + shaft * shaft, vec3(1.0 / 2.2));
    }
#endif

    outColor = vec4(color, 1.0);
}
