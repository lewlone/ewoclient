#version 330 compatibility

/*
    shadow.vsh — the world rendered from the sun/moon into the shadow map.
    Iris supplies the light's model-view/projection; we only add the
    resolution-concentrating distortion (see lib/shadow_distort.glsl).
*/

#include "/lib/shadow_distort.glsl"

out vec2 texcoord;
out vec4 glcolor;

void main() {
    gl_Position = ftransform();
    gl_Position.xyz = distortShadow(gl_Position.xyz);
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    glcolor = gl_Color;
}
