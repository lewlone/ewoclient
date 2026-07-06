#version 330 compatibility

/*
    composite1.fsh — EwoWater 2.0 composite + atmospheric fog.

    Order of operations (each stage feeds the next):

      1. BLEND LAYERS   colortex5 (translucency accumulation, written by
                        the clrwl/legacy geometry programs, premultiplied)
                        is composited over the opaque scene — 26.2's
                        pipeline doesn't do this for us.
      2. WATER          water pixels (colortex4 mask) are repainted:
                        absorption-graded body color from TRUE water depth
                        (seafloor distance from depthtex0 minus surface
                        distance recorded in colortex4.g) — turquoise
                        shallows to deep navy with zero transparency —
                        plus pearl shore foam from the same depth signal.
      3. MIRROR         fresnel-weighted reflection: screen-space DDA
                        march (below-plane hits rejected, travel-scaled
                        blur) with the analytic Velvet sky as fallback,
                        ripple wobble, and the sun glint.
      4. FOG            distance dissolves toward the shared sky model.
      5. UNDERWATER     submerged camera gets velvet-teal fog + tint.

    The bloom chain (composite2..4) runs after; final tonemaps.
*/

#include "/lib/settings.glsl"
#include "/lib/sky.glsl"

uniform sampler2D colortex0;
uniform sampler2D colortex3;
uniform sampler2D colortex4;
uniform sampler2D colortex5;
uniform sampler2D depthtex0;
uniform sampler2D depthtex1; // opaque-only: under water this is ALWAYS the seafloor

uniform mat4 gbufferProjection;
uniform mat4 gbufferProjectionInverse;
uniform mat4 gbufferModelView;
uniform mat4 gbufferModelViewInverse;

uniform vec3 sunPosition;
uniform vec3 cameraPosition;
uniform vec3 skyColor;
uniform vec3 fogColor;
uniform float near;
uniform float far;
uniform float viewWidth;
uniform float viewHeight;
uniform float rainStrength;
uniform int isEyeInWater;

in vec2 texcoord;

/* RENDERTARGETS: 0 */
layout(location = 0) out vec4 outColor;

// ── Helpers ─────────────────────────────────────────────────────────────

float ign(vec2 fragCoord) {
    return fract(52.9829189 * fract(dot(fragCoord, vec2(0.06711056, 0.00583715))));
}

vec3 viewPosFromDepth(vec2 uv, float depth) {
    vec4 clip = vec4(vec3(uv, depth) * 2.0 - 1.0, 1.0);
    vec4 view = gbufferProjectionInverse * clip;
    return view.xyz / view.w;
}

vec3 viewToScreen(vec3 viewPos) {
    vec4 clip = gbufferProjection * vec4(viewPos, 1.0);
    return (clip.xyz / clip.w) * 0.5 + 0.5;
}

// Screen-space DDA raymarch — uniform UV steps, perspective-correct depth
// via 1/z, crossing-detect + refine, reject-and-continue past occluders.
// Deterministic (no per-pixel jitter: neighbours must agree or thin
// features turn to salt-and-pepper). Returns vec4(uv, hit, travelDist).
vec4 raymarchSSR(vec3 originView, vec3 dirView) {
    float maxLen = 160.0;
    if (dirView.z > 0.0) {
        float toNear = (-near - originView.z) / dirView.z;
        maxLen = min(maxLen, toNear * 0.95);
        if (maxLen <= 0.5) {
            return vec4(0.0);
        }
    }
    vec3 endView = originView + dirView * maxLen;
    vec3 s0 = viewToScreen(originView);
    vec3 s1 = viewToScreen(endView);
    vec2 duv = s1.xy - s0.xy;
    float invZ0 = 1.0 / originView.z;
    float invZ1 = 1.0 / endView.z;

    const int STEPS = 80;
    float prevT = 0.0;
    for (int i = 1; i <= STEPS; i++) {
        float t = (float(i) - 0.5) / float(STEPS);
        vec2 uv = s0.xy + duv * t;
        if (clamp(uv, 0.0, 1.0) != uv) {
            break;
        }
        float rayZ = 1.0 / mix(invZ0, invZ1, t);
        float sceneZ = viewPosFromDepth(uv, texture(depthtex0, uv).r).z;
        if (rayZ < sceneZ - 0.02) {
            float lo = prevT;
            float hi = t;
            for (int j = 0; j < 5; j++) {
                float m = (lo + hi) * 0.5;
                vec2 muv = s0.xy + duv * m;
                float mz = 1.0 / mix(invZ0, invZ1, m);
                float sz = viewPosFromDepth(muv, texture(depthtex0, muv).r).z;
                if (mz < sz - 0.02) { hi = m; } else { lo = m; }
            }
            float ft = (lo + hi) * 0.5;
            vec2 fuv = s0.xy + duv * ft;
            float frayZ = 1.0 / mix(invZ0, invZ1, ft);
            float fsceneZ = viewPosFromDepth(fuv, texture(depthtex0, fuv).r).z;
            float tolerance = max(0.8, -fsceneZ * 0.12);
            if (frayZ - fsceneZ > -tolerance) {
                vec3 hitView = viewPosFromDepth(fuv, texture(depthtex0, fuv).r);
                return vec4(fuv, 1.0, length(hitView - originView));
            }
        }
        prevT = t;
    }
    return vec4(0.0);
}

// ── Water body color ────────────────────────────────────────────────────
// Absorption gradient from true water depth: turquoise shallows sinking
// to velvet navy. Opaque — depth is ENCODED in color, never see-through.

vec3 waterBodyColor(float waterDepth, float skyLight) {
    const vec3 SHALLOW = vec3(0.28, 0.52, 0.55);
    const vec3 DEEP = vec3(0.09, 0.16, 0.33);
    float t = 1.0 - exp(-waterDepth * 0.22);
    float exposure = mix(0.22, 1.0, smoothstep(0.1, 0.9, skyLight));
    return mix(SHALLOW * 0.75, DEEP, t) * exposure;
}

void main() {
    vec3 color = texture(colortex0, texcoord).rgb;
    float depth = texture(depthtex0, texcoord).r;

    // 1 ── BLEND LAYERS
    vec4 trans = texture(colortex5, texcoord);
    color = trans.rgb + color * (1.0 - trans.a);

    vec3 sunDirWorld = normalize(mat3(gbufferModelViewInverse) * sunPosition);
    float wet = 1.0 - rainStrength;

    vec4 wmask = texture(colortex4, texcoord);
    if (wmask.r > 0.5) {
        // ── Surface reconstruction (water writes no depth on 26.2:
        // depthtex0 here is the SEAFLOOR; the surface distance was
        // recorded by the geometry stage).
        vec3 rayDirView = normalize(viewPosFromDepth(texcoord, 0.5));
        float surfaceDist = wmask.g;
        vec3 viewPos = rayDirView * surfaceDist;

        vec4 gdata = texture(colortex3, texcoord);
        vec3 worldNormal = normalize(gdata.rgb * 2.0 - 1.0);
        float skyLight = gdata.a;

        // depthtex1 (opaque-only), NOT depthtex0: whether translucents
        // write depth varies by pipeline (they DO under colorwheel/OIT —
        // learned the hard way when depthtex0 gave depth 0 and foam
        // covered the entire ocean). depthtex1 excludes translucents by
        // definition, so it is the seafloor on every pipeline.
        float seafloorDist = length(viewPosFromDepth(texcoord, texture(depthtex1, texcoord).r));
        float waterDepth = max(seafloorDist - surfaceDist, 0.0);

        // 2 ── WATER BODY + FOAM
        color = waterBodyColor(waterDepth, skyLight);

        float foamBand = 0.0;
#if WATER_FOAM == 1
        // Pearl foam where the water thins against land — with a LAND
        // ADJACENCY requirement: "shallow" alone also fires on opaque
        // geometry just under the surface, i.e. every mid-ocean KELP TOP
        // (they reach within a block of the surface and sit in the depth
        // buffer) — which painted bright kelp-shaped foam patches that
        // read as kelp poking through the water, shimmering with the
        // ripple-driven foam noise. The whole "kelp saga" was this foam.
        if (waterDepth < 1.1) {
            float surfWorldY = (gbufferModelViewInverse * vec4(viewPos, 1.0)).y + cameraPosition.y;
            vec2 ps7 = 7.0 / vec2(viewWidth, viewHeight);
            const vec2 dirs[8] = vec2[](
                vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0),
                vec2(0.7, 0.7), vec2(-0.7, 0.7), vec2(0.7, -0.7), vec2(-0.7, -0.7));
            bool nearLand = false;
            for (int i = 0; i < 8; i++) {
                vec2 nuv = clamp(texcoord + dirs[i] * ps7, vec2(0.0), vec2(1.0));
                if (texture(colortex4, nuv).r < 0.5) {
                    vec3 nView = viewPosFromDepth(nuv, texture(depthtex1, nuv).r);
                    float nY = (gbufferModelViewInverse * vec4(nView, 1.0)).y + cameraPosition.y;
                    if (nY > surfWorldY - 0.35) {
                        nearLand = true;
                        break;
                    }
                }
            }
            if (nearLand) {
                float foamNoise = 0.75 + 0.25 * worldNormal.x * 8.0;
                foamBand = smoothstep(1.1, 0.12, waterDepth) * clamp(foamNoise, 0.4, 1.0);
                vec3 foamColor = vec3(0.93, 0.90, 0.92)
                               * mix(0.3, 1.0, smoothstep(0.1, 0.9, skyLight));
                color = mix(color, foamColor, foamBand * 0.85);
            }
        }
#endif

        // 3 ── MIRROR
        float wobbleAmt = mix(0.85, 0.10, smoothstep(20.0, 110.0, surfaceDist));
        vec3 viewDirWorld = normalize(mat3(gbufferModelViewInverse) * viewPos);
        vec3 wobbleDir = reflect(viewDirWorld, normalize(mix(vec3(0.0, 1.0, 0.0), worldNormal, wobbleAmt)));
        wobbleDir = normalize(vec3(wobbleDir.x, max(wobbleDir.y, 0.02), wobbleDir.z));
        vec3 skyRefl = skyVelvet(wobbleDir, sunDirWorld, skyColor, fogColor);

        float flatten = mix(0.35, 0.08, smoothstep(8.0, 60.0, surfaceDist));
        vec3 rayNormalW = normalize(mix(vec3(0.0, 1.0, 0.0), worldNormal, flatten));
        vec3 viewNormal = normalize(mat3(gbufferModelView) * rayNormalW);
        vec3 viewDir = normalize(viewPos);
        vec3 reflDir = reflect(viewDir, viewNormal);

        vec3 reflWorldDir = normalize(mat3(gbufferModelViewInverse) * reflDir);
        if (reflWorldDir.y < 0.015) {
            reflWorldDir = normalize(vec3(reflWorldDir.x, 0.015, reflWorldDir.z));
            reflDir = normalize(mat3(gbufferModelView) * reflWorldDir);
        }

        float ndotv = clamp(dot(-viewDir, viewNormal), 0.0, 1.0);
        float fresnel = 0.02 + 0.98 * pow(1.0 - ndotv, 5.0);

        vec3 refl = skyRefl;
        vec4 hit = vec4(0.0);
#if WATER_SSR == 1
        hit = raymarchSSR(viewPos, reflDir);
#endif
        if (hit.z > 0.5) {
            // THE critical rejection — hits that land on WATER pixels.
            // Under the OIT pipeline our water color lives in colortex5;
            // colortex0 at water locations holds the RAW SEAFLOOR (opaque
            // pass only). Grazing rays riding the animated ripple normals
            // skim the plane and "hit" the water surface nearby — sampling
            // colortex0 there projected the seabed onto the surface: the
            // see-through illusion that moved with the waves and got worse
            // near the surface. Water-hits-water resolves to the sky model
            // instead (which is what distant water mirrors anyway).
            if (texture(colortex4, hit.xy).r > 0.5) {
                hit.z = 0.0;
            }
        }
        if (hit.z > 0.5) {
            // Below-plane rejection: the seafloor is real geometry in the
            // depth buffer; a ray above water can't legitimately hit
            // beneath it.
            vec3 hitView = viewPosFromDepth(hit.xy, texture(depthtex0, hit.xy).r);
            float hitWorldY = (gbufferModelViewInverse * vec4(hitView, 1.0)).y + cameraPosition.y;
            float surfWorldY = (gbufferModelViewInverse * vec4(viewPos, 1.0)).y + cameraPosition.y;
            if (hitWorldY < surfWorldY - 0.4) {
                hit.z = 0.0;
            }
        }
        if (hit.z > 0.5) {
            vec2 border = min(hit.xy, 1.0 - hit.xy);
            float edgeFade = smoothstep(0.0, 0.06, min(border.x, border.y));
            vec2 ps = 1.0 / vec2(viewWidth, viewHeight);
            float blurR = clamp(hit.w * 0.08, 0.75, 5.0);
            vec3 hitColor = texture(colortex0, hit.xy).rgb * 0.4
                + texture(colortex0, hit.xy + vec2(ps.x, 0.0) * blurR).rgb * 0.15
                + texture(colortex0, hit.xy - vec2(ps.x, 0.0) * blurR).rgb * 0.15
                + texture(colortex0, hit.xy + vec2(0.0, ps.y) * blurR).rgb * 0.15
                + texture(colortex0, hit.xy - vec2(0.0, ps.y) * blurR).rgb * 0.15;
            float pathLen = surfaceDist + hit.w;
            float pathFog = 1.0 - exp(-pow(pathLen / (far * FOG_DISTANCE), 1.8));
            hitColor = mix(hitColor, skyRefl, clamp(pathFog, 0.0, 1.0));
            refl = mix(skyRefl, hitColor, edgeFade);
        }

        float mirror = clamp(max(fresnel * SSR_STRENGTH, 0.12), 0.0, 0.9);
        mirror *= (1.0 - foamBand); // foam is matte
        color = mix(color, refl, mirror);

        // Sun glint on the full ripple normal.
        float glint = pow(clamp(dot(reflect(viewDirWorld, worldNormal), sunDirWorld), 0.0, 1.0), 480.0);
        float day = smoothstep(-0.05, 0.15, sunDirWorld.y);
        color += lightColorVelvet(sunDirWorld) * glint * 2.5
               * fresnel * day * wet * (1.0 - foamBand);
    }

    // 3.5 ── WATERLINE GAP FILL. At extreme grazing the water plane
    // projects below one pixel of height and the rasterizer drops it,
    // while vertical geometry behind it (kelp, seagrass) still draws —
    // so underwater plants appear to poke through the surface along a
    // 1–3px seam. Detect: this pixel isn't water, but water exists just
    // below on screen AND our opaque hit sits below that water's plane —
    // i.e. the ray should have crossed the surface first. Paint it as
    // grazing water: at these angles that's the horizon mirror.
    if (wmask.r < 0.5 && wmask.b > 0.5 && isEyeInWater == 0) {
        // EXACT seam detection: this pixel is a submerged-only plant
        // (kelp/seagrass — block-ID tagged; they cannot exist outside
        // water) with NO water fragment in front of it. That is a
        // waterline rasterization seam by definition — unbounded range,
        // angle-independent. Paint as grazing water.
        vec3 dirW = normalize(mat3(gbufferModelViewInverse) * viewPosFromDepth(texcoord, 0.5));
        vec3 mirrorDir = normalize(vec3(dirW.x, max(-dirW.y, 0.02), dirW.z));
        color = skyVelvet(mirrorDir, sunDirWorld, skyColor, fogColor);
    } else if (wmask.r < 0.5 && depth < 1.0) {
        // Strided probe, ~94px reach at 32 taps: fallback for untagged
        // below-plane geometry at seams. Breaks at first water found; its
        // plane height anchors the below-plane test.
        vec2 ps = 1.0 / vec2(viewWidth, viewHeight);
        for (int i = 1; i <= 32; i++) {
            vec2 nuv = texcoord - vec2(0.0, ps.y * (float(i) * 3.0 - 2.0));
            if (nuv.y < 0.0) { break; }
            vec4 nmask = texture(colortex4, nuv);
            if (nmask.r > 0.5) {
                // Neighbor water surface world height.
                vec3 nDir = normalize(viewPosFromDepth(nuv, 0.5));
                vec3 nSurfView = nDir * nmask.g;
                float nSurfY = (gbufferModelViewInverse * vec4(nSurfView, 1.0)).y + cameraPosition.y;
                // Our own opaque hit height (kelp is cutout → opaque).
                vec3 hitView = viewPosFromDepth(texcoord, texture(depthtex1, texcoord).r);
                float hitY = (gbufferModelViewInverse * vec4(hitView, 1.0)).y + cameraPosition.y;
                if (hitY < nSurfY - 0.05) {
                    vec3 dirW = normalize(mat3(gbufferModelViewInverse) * viewPosFromDepth(texcoord, 0.5));
                    vec3 mirrorDir = normalize(vec3(dirW.x, max(-dirW.y, 0.02), dirW.z));
                    color = skyVelvet(mirrorDir, sunDirWorld, skyColor, fogColor);
                }
                break;
            }
        }
    }

    // 4 ── ATMOSPHERIC FOG (water uses its surface distance, not seafloor)
#if VELVET_FOG == 1
    if (depth < 1.0 || wmask.r > 0.5) {
        float fogDist = wmask.r > 0.5 ? wmask.g : length(viewPosFromDepth(texcoord, depth));
        vec3 dirWorld = normalize(mat3(gbufferModelViewInverse) * viewPosFromDepth(texcoord, 0.5));
        float fogAmount = 1.0 - exp(-pow(fogDist / (far * FOG_DISTANCE), 1.8));
        vec3 fogTo = skyVelvet(dirWorld, sunDirWorld, skyColor, fogColor);
        color = mix(color, fogTo, clamp(fogAmount, 0.0, 1.0));
    }
#endif

    // 5 ── UNDERWATER: submerged camera gets velvet-teal medium fog.
    if (isEyeInWater == 1) {
        float sceneDist = depth < 1.0 ? length(viewPosFromDepth(texcoord, depth)) : far;
        if (wmask.r > 0.5) {
            sceneDist = min(sceneDist, wmask.g);
        }
        vec3 waterFog = vec3(0.10, 0.24, 0.32);
        float fogAmount = 1.0 - exp(-sceneDist * 0.09);
        color = mix(color, waterFog, clamp(fogAmount, 0.0, 1.0));
        color *= vec3(0.82, 0.95, 1.0); // teal cast on what remains
    }

    outColor = vec4(color, 1.0);
}
