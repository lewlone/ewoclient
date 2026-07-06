#version 330 compatibility

/*
    gbuffers_hand.fsh — texture × color × lightmap, opaque. Writes RT3
    with skylight 0 so the composite shading pass leaves the hand alone
    (world-space shadows on a view-space hand look wrong anyway).
*/

uniform sampler2D gtexture;
uniform sampler2D lightmap;
uniform float alphaTestRef;

in vec2 texcoord;
in vec2 lmcoord;
in vec4 glcolor;

/* RENDERTARGETS: 0,3 */
layout(location = 0) out vec4 color;
layout(location = 1) out vec4 data;

void main() {
    color = texture(gtexture, texcoord) * glcolor;
    color.rgb *= texture(lightmap, lmcoord).rgb;
    if (color.a < alphaTestRef) {
        discard;
    }
    color.a = 1.0;
    data = vec4(0.5, 0.5, 0.5, 0.0);
}
