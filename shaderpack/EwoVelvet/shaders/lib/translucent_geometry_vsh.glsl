#ifndef TRANSLUCENT_GEOMETRY_VSH_GLSL
#define TRANSLUCENT_GEOMETRY_VSH_GLSL

/*
    EwoWater 2.0 — shared vertex body for the two translucent geometry
    programs (clrwl_gbuffers_translucent = 26.2 colorwheel pipeline,
    gbuffers_water = legacy route). The including file provides only the
    #version line and the include.
*/

uniform mat4 gbufferModelViewInverse;
uniform vec3 cameraPosition;
uniform float frameTimeCounter;

in vec4 mc_Entity; // block id via block.properties (10001 = water)

out vec2 texcoord;
out vec2 lmcoord;
out vec4 glcolor;
out vec3 worldNormal;
out vec3 worldPosFrag; // world coords — continuous ripple domain
out vec3 viewPosFrag;  // model-view position — camera-relative by definition
out float waterMask;   // any water face (block id)
out float waveMask;    // up-facing water: ripples + vertex bob
out float plantMask;   // submerged-only plants (kelp/seagrass, id 10002)

void main() {
    vec4 pos = gl_ModelViewMatrix * gl_Vertex;
    vec3 world = (gbufferModelViewInverse * pos).xyz + cameraPosition;
    vec3 normal = mat3(gbufferModelViewInverse) * (gl_NormalMatrix * gl_Normal);

    waterMask = (mc_Entity.x > 10000.5 && mc_Entity.x < 10001.5) ? 1.0 : 0.0;
    plantMask = (mc_Entity.x > 10001.5 && mc_Entity.x < 10002.5) ? 1.0 : 0.0;
    waveMask = 0.0;
#if WATER_WAVES == 1
    if (waterMask > 0.5 && normal.y > 0.9) {
        waveMask = 1.0;
    }
#endif
    // NO vertex displacement — ever. Sodium greedy-meshes water into
    // variable-size quads; per-vertex displacement makes shared edges
    // disagree, opening ANIMATED CRACKS in the surface. At grazing angles
    // those slits align with the view and the seafloor shows through —
    // the "transparency that moves with the waves and starts closer when
    // the camera is lower" that resisted every fragment-side fix. Waves
    // live in the normal field only (shading/reflections/glint); the
    // geometry stays watertight by construction.

    gl_Position = gl_ProjectionMatrix * pos;
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    lmcoord = (gl_TextureMatrix[1] * gl_MultiTexCoord1).xy;
    glcolor = gl_Color;
    worldNormal = normal;
    worldPosFrag = world;
    viewPosFrag = pos.xyz;
}

#endif // TRANSLUCENT_GEOMETRY_VSH_GLSL
