#version 330 compatibility

/*
    gbuffers_skybasic.fsh — the Velvet sky. Replaces vanilla's flat
    two-tone disc with the shared skyVelvet gradient (see lib/sky.glsl).
    Stars come through the same program as tiny bright quads — detected
    via Iris's renderStage and passed through with a pearl lift so the
    night sky keeps its jewels.
*/

#include "/lib/settings.glsl"
#include "/lib/sky.glsl"

uniform mat4 gbufferModelViewInverse;
uniform vec3 sunPosition;
uniform vec3 skyColor;
uniform vec3 fogColor;
uniform int renderStage;

in vec3 viewPos;
in vec4 glcolor;

/* RENDERTARGETS: 0 */
layout(location = 0) out vec4 outColor;

void main() {
#if VELVET_SKY == 1
    if (renderStage == MC_RENDER_STAGE_STARS) {
        // Stars: vanilla feeds brightness through glcolor. Pearl-warm it.
        outColor = vec4(vec3(0.96, 0.93, 0.98) * glcolor.rgb * 1.35, glcolor.a);
        return;
    }
    vec3 dirWorld = normalize(mat3(gbufferModelViewInverse) * viewPos);
    vec3 sunDirWorld = normalize(mat3(gbufferModelViewInverse) * sunPosition);
    outColor = vec4(skyVelvet(dirWorld, sunDirWorld, skyColor, fogColor), 1.0);
#else
    outColor = glcolor;
#endif
}
