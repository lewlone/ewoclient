#version 330 compatibility

/* composite4.fsh — bloom blur, vertical leg. colortex2 → colortex1. */

#include "/lib/settings.glsl"
#include "/lib/bloom_blur.glsl"

uniform sampler2D colortex2;
uniform float viewWidth;
uniform float viewHeight;

in vec2 texcoord;

/* RENDERTARGETS: 1 */
layout(location = 0) out vec4 blurred;

void main() {
#if BLOOM == 1
    vec2 pixelSize = 1.0 / vec2(viewWidth, viewHeight);
    blurred = vec4(bloomBlur(colortex2, texcoord, vec2(0.0, 1.0), pixelSize), 1.0);
#else
    blurred = vec4(0.0);
#endif
}
