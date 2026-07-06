#version 330 compatibility

/*
    shadow.fsh — cutout alpha test so leaves cast leafy shadows, plus the
    surface color into shadowcolor0 (unused today; reserved for colored
    glass/water shadows later).
*/

uniform sampler2D gtexture;
uniform float alphaTestRef;

in vec2 texcoord;
in vec4 glcolor;

layout(location = 0) out vec4 color;

void main() {
    color = texture(gtexture, texcoord) * glcolor;
    if (color.a < alphaTestRef) {
        discard;
    }
}
