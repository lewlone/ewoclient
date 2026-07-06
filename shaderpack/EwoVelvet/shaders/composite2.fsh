#version 330 compatibility

/*
    composite2.fsh — bloom bright-pass (runs on the shaded + reflected +
    fogged scene). Extracts everything above BLOOM_THRESHOLD with a soft
    knee into colortex1, linear, warmed toward pearl.

    Buffer formats for the whole pipeline are declared here.
*/

/*
const int colortex1Format = RGBA16F;
const int colortex2Format = RGBA16F;
const int colortex3Format = RGBA16F;
const int colortex4Format = RGBA16F;
const int colortex5Format = RGBA16F;
*/

#include "/lib/settings.glsl"

uniform sampler2D colortex0;

in vec2 texcoord;

/* RENDERTARGETS: 1 */
layout(location = 0) out vec4 bright;

void main() {
#if BLOOM == 1
    vec3 color = texture(colortex0, texcoord).rgb;
    vec3 lin = pow(color, vec3(2.2));

    float luma = dot(lin, vec3(0.2126, 0.7152, 0.0722));
    float knee = BLOOM_THRESHOLD * 0.25;
    float weight = smoothstep(BLOOM_THRESHOLD - knee, BLOOM_THRESHOLD + knee, luma);

    vec3 glow = lin * weight;
    glow = mix(glow, luma * weight * pow(VELVET_HIGHLIGHT_TINT, vec3(2.2)), BLOOM_PEARL_TINT);

    bright = vec4(glow, 1.0);
#else
    bright = vec4(0.0);
#endif
}
