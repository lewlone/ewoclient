#ifndef TERRAIN_GEOMETRY_FSH_GLSL
#define TERRAIN_GEOMETRY_FSH_GLSL

/*
    Shared fragment body for the opaque terrain programs (legacy
    gbuffers_terrain + clrwl_gbuffers). Atlas × biome/vertex color ×
    lightmap with cutout alpha test, plus the G-data the composite passes
    consume: colortex3 (world normal + skylight), colortex4.b (submerged-
    plant flag — water overwrites it wherever it covers the plant, so a
    surviving flag with no water mask marks a waterline seam pixel).
*/

uniform sampler2D gtexture;
uniform sampler2D lightmap;
uniform float alphaTestRef;

in vec2 texcoord;
in vec2 lmcoord;
in vec4 glcolor;
in vec3 worldNormal;
in float plantFlag;

/* RENDERTARGETS: 0,3,4 */
layout(location = 0) out vec4 color;
layout(location = 1) out vec4 data;
layout(location = 2) out vec4 mask;

void main() {
    color = texture(gtexture, texcoord) * glcolor;
    color.rgb *= texture(lightmap, lmcoord).rgb;
    if (color.a < alphaTestRef) {
        discard;
    }
    data = vec4(normalize(worldNormal) * 0.5 + 0.5, lmcoord.y);
    mask = vec4(0.0, 0.0, plantFlag, 1.0);
}

#endif // TERRAIN_GEOMETRY_FSH_GLSL
