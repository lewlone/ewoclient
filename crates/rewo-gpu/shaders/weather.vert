#version 450
// Rain and snow — `core/particle.vsh` as `WeatherEffectRenderer` uses it.
//
//     gl_Position = ProjMat * ModelViewMat * vec4(Position, 1.0);
//     texCoord0   = UV0;
//     vertexColor = Color * sample_lightmap(Sampler2, UV2);
//
// Vanilla's weather uses `DefaultVertexFormat.PARTICLE`, whose colour is always
// `ARGB.white(alpha)` here — so only the alpha varies per column and the rest
// is a plain lightmap multiply. The positions arrive already **camera-relative**
// (the CPU subtracts the camera), so the model-view is a pure rotation.

#include "lightmap.glsl"

layout(push_constant) uniform PC {
    mat4 mvp;
    // `[SkyFactor, BlockFactor, BrightnessFactor, DarknessScale]` and
    // `[SkyLightColor.rgb, NightVisionFactor]`, exactly as the world pass packs
    // them — so weather is lit by the same resolved lightmap as terrain.
    vec4 light;
    vec4 sky_col;
    vec4 ambient;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in float in_alpha;
// Block level in bits 16..19 and sky in 20..23, the word shape `lm_light`
// expects — the same packing the mesher uses for terrain.
layout(location = 3) in uint in_light;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    gl_Position = pc.mvp * vec4(in_pos, 1.0);
    v_uv = in_uv;
    vec3 lm = lm_light(in_light, pc.light, pc.sky_col, pc.ambient.rgb);
    v_color = vec4(lm, in_alpha);
}
