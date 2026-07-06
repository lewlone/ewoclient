#version 330 compatibility

/*
    gbuffers_entities.vsh — entities (mobs, players, item frames, armor
    stands). Shipped because Iris 1.11.1's built-in entity fallback on
    MC 26.2 renders entities semi-transparent (broken blend state) — same
    family of 26.2 fallback breakage as the atlas-less terrain. Writing
    RT3 here also opts entities into the composite sun shading, so they
    receive shadows like terrain does.
*/

uniform mat4 gbufferModelViewInverse;

out vec2 texcoord;
out vec2 lmcoord;
out vec4 glcolor;
out vec3 worldNormal;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    lmcoord = (gl_TextureMatrix[1] * gl_MultiTexCoord1).xy;
    glcolor = gl_Color;
    worldNormal = mat3(gbufferModelViewInverse) * (gl_NormalMatrix * gl_Normal);
}
