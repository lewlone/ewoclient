#version 330 compatibility

/*
    final.vsh — fullscreen pass vertex stage.

    Iris draws one screen-covering quad for composite/final programs; all
    the interesting work happens in the fragment stage. This just forwards
    position + UV in the documented Iris style.
*/

out vec2 texcoord;

void main() {
    gl_Position = ftransform();
    texcoord = (gl_TextureMatrix[0] * gl_MultiTexCoord0).xy;
}
