#version 330 compatibility

/*
    gbuffers_hand.vsh — first-person hand + held item. Shipped because the
    26.2 built-in fallback renders the hand semi-transparent (same blend
    breakage as entities).
*/

out vec2 texcoord;
out vec2 lmcoord;
out vec4 glcolor;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
    lmcoord = (gl_TextureMatrix[1] * gl_MultiTexCoord1).xy;
    glcolor = gl_Color;
}
