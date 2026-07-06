#version 330 compatibility

/* composite3.fsh — bloom blur, horizontal leg. colortex1 → colortex2. */

#include "/lib/settings.glsl"
#include "/lib/bloom_blur.glsl"

uniform sampler2D colortex1;
uniform float viewWidth;
uniform float viewHeight;

in vec2 texcoord;

/* RENDERTARGETS: 2 */
layout(location = 0) out vec4 blurred;

void main() {
#if BLOOM == 1
    vec2 pixelSize = 1.0 / vec2(viewWidth, viewHeight);
    blurred = vec4(bloomBlur(colortex1, texcoord, vec2(1.0, 0.0), pixelSize), 1.0);
#else
    blurred = vec4(0.0);
#endif
}
