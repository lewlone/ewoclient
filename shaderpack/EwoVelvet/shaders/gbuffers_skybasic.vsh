#version 330 compatibility

/*
    gbuffers_skybasic.vsh — vanilla's sky disc / void plane / stars
    geometry. Forwards the view-space position so the fragment stage can
    compute a world direction per pixel for the Velvet sky gradient.
*/

out vec3 viewPos;
out vec4 glcolor;

void main() {
    gl_Position = ftransform();
    viewPos = (gl_ModelViewMatrix * gl_Vertex).xyz;
    glcolor = gl_Color;
}
