#ifndef TERRAIN_GEOMETRY_VSH_GLSL
#define TERRAIN_GEOMETRY_VSH_GLSL

/*
    Shared vertex body for the opaque terrain programs: legacy
    gbuffers_terrain AND clrwl_gbuffers (26.2 colorwheel opaque path —
    cutout foliage like kelp routes through colorwheel; without a clrwl
    program it renders built-in and escapes every pack feature).
*/

uniform mat4 gbufferModelViewInverse;

in vec4 mc_Entity; // block id via block.properties (10002 = submerged plant)

out vec2 texcoord;
out vec2 lmcoord;
out vec4 glcolor;
out vec3 worldNormal;
out float plantFlag;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    lmcoord = (gl_TextureMatrix[1] * gl_MultiTexCoord1).xy;
    glcolor = gl_Color;
    worldNormal = mat3(gbufferModelViewInverse) * (gl_NormalMatrix * gl_Normal);
    plantFlag = (mc_Entity.x > 10001.5 && mc_Entity.x < 10002.5) ? 1.0 : 0.0;
}

#endif // TERRAIN_GEOMETRY_VSH_GLSL
