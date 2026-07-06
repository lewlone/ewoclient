#ifndef TRANSLUCENT_GEOMETRY_FSH_GLSL
#define TRANSLUCENT_GEOMETRY_FSH_GLSL

/*
    EwoWater 2.0 — shared fragment body for the two translucent geometry
    programs. The G-data contract lives here and ONLY here:

      colortex5  premultiplied surface color (accumulation / OIT target)
      colortex3  world normal (rippled for water) + skylight in .a
      colortex4  r: waterMask  g: surface view distance  b: blocklight

    Water: opaque, minimal analytic base (lib/water_surface.glsl) — the
    composite pass owns its real appearance. Non-water translucents
    (glass, ice, slime): vanilla look, translucent, no water G-data.
*/

#include "/lib/water_surface.glsl"

uniform sampler2D gtexture;
uniform sampler2D lightmap;
uniform float alphaTestRef;
uniform float frameTimeCounter;
uniform int isEyeInWater;

in vec2 texcoord;
in vec2 lmcoord;
in vec4 glcolor;
in vec3 worldNormal;
in vec3 worldPosFrag;
in vec3 viewPosFrag;
in float waterMask;
in float waveMask;
in float plantMask;

/* RENDERTARGETS: 5,3,4 */
layout(location = 0) out vec4 outColor;
layout(location = 1) out vec4 outData;
layout(location = 2) out vec4 outMask;

void main() {
    float viewDist = length(viewPosFrag);

    // Submerged plants (kelp/seagrass) render through THIS translucent
    // pass on 26.2 — and used to stamp r=0 over the water's mask (OIT
    // fragment order is unordered, so they'd win in shifting silhouettes)
    // while their bright tips rode the accumulation over the water color:
    // the "kelp poking through" family of artifacts. Our water is opaque,
    // so from above these plants are simply not visible — discard. From
    // underwater they render normally.
    if (plantMask > 0.5) {
        if (isEyeInWater != 1) {
            discard;
        }
        vec4 c = texture(gtexture, texcoord) * glcolor;
        c.rgb *= texture(lightmap, lmcoord).rgb;
        if (c.a < alphaTestRef) {
            discard;
        }
        outColor = vec4(c.rgb * c.a, c.a);
        outData = vec4(0.5, 0.5, 0.5, 0.0);
        outMask = vec4(0.0, viewDist, 1.0, 1.0); // b: plant flag
        return;
    }

    if (waterMask > 0.5) {
        // Water — opaque velvet anchor; composite repaints it.
        outColor = ewoWaterBase(lmcoord.y);
        vec3 normal = normalize(worldNormal);
#if WATER_WAVES == 1
        if (waveMask > 0.5) {
            normal = ewoWaterNormal(worldPosFrag.xz, frameTimeCounter, viewDist);
        }
#endif
        outData = vec4(normal * 0.5 + 0.5, lmcoord.y);
        outMask = vec4(1.0, viewDist, lmcoord.x, 1.0);
    } else {
        // Glass / ice / slime / nether portal — vanilla appearance,
        // genuinely translucent, premultiplied for the accumulation blend.
        vec4 c = texture(gtexture, texcoord) * glcolor;
        c.rgb *= texture(lightmap, lmcoord).rgb;
        if (c.a < alphaTestRef) {
            discard;
        }
        outColor = vec4(c.rgb * c.a, c.a);
        // No water G-data — "not water, not plant" mask.
        outData = vec4(0.5, 0.5, 0.5, 0.0);
        outMask = vec4(0.0, viewDist, 0.0, 1.0);
    }

    // Premultiply water too (alpha 1 → no-op, kept for uniformity).
    outColor.rgb *= 1.0; // (already premultiplied where needed above)
}

#endif // TRANSLUCENT_GEOMETRY_FSH_GLSL
