#version 330 compatibility

/*
    gbuffers_entities.fsh — entity texture × tint × lightmap, opaque
    (alpha-tested, never blended). `entityColor` is the damage/creeper
    flash overlay vanilla applies (red on hurt).
*/

uniform sampler2D gtexture;
uniform sampler2D lightmap;
uniform vec4 entityColor;
uniform float alphaTestRef;

in vec2 texcoord;
in vec2 lmcoord;
in vec4 glcolor;
in vec3 worldNormal;

/* RENDERTARGETS: 0,3 */
layout(location = 0) out vec4 color;
layout(location = 1) out vec4 data;

void main() {
    // Kill interior surfaces: vanilla draws the outer skin layer
    // double-sided, so with the camera inside the model (FreeLook parks
    // it in the head) you'd see the inside of your own face. Back-facing
    // fragments are never visible from outside, so this is free there.
    if (!gl_FrontFacing) {
        discard;
    }
    color = texture(gtexture, texcoord) * glcolor;
    color.rgb = mix(color.rgb, entityColor.rgb, entityColor.a);
    color.rgb *= texture(lightmap, lmcoord).rgb;
    if (color.a < alphaTestRef) {
        discard;
    }
    // Entities are opaque past the alpha test — force it so no stale
    // blend state can make mobs translucent (the bug this file fixes).
    color.a = 1.0;
    data = vec4(normalize(worldNormal) * 0.5 + 0.5, lmcoord.y);
}
