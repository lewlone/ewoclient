#version 450
// Particles — `assets/minecraft/shaders/core/particle.vsh` (M35).
//
//     gl_Position = ProjMat * ModelViewMat * vec4(Position, 1.0);
//     texCoord0   = UV0;
//     vertexColor = Color * sample_lightmap(Sampler2, UV2);
//
// The same shader vanilla's weather uses, which is why this file reads almost
// identically to `weather.vert` — vanilla draws rain through the particle
// pipeline. The one difference that matters: weather's `Color` is always
// `ARGB.white(alpha)`, so only alpha varies per vertex, while a real particle
// carries a full RGBA (a crit fades toward red as `gCol`/`bCol` decay, a
// terrain shard is tinted by its block).
//
// Positions arrive in WORLD space. Vanilla's are camera-relative because its
// model-view carries the camera translation; Rewo's `view_proj` already
// includes it, so emitting the relative form would pile every particle at the
// world origin — the same trap M33 hit with weather geometry.

#include "lightmap.glsl"

layout(push_constant) uniform PC {
    mat4 mvp;
    // `[SkyFactor, BlockFactor, BrightnessFactor, DarknessScale]` and
    // `[SkyLightColor.rgb, NightVisionFactor]`, packed as the world pass packs
    // them, so a particle is lit by the same resolved lightmap as terrain.
    vec4 light;
    vec4 sky_col;
    vec4 ambient;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;
// Block level in bits 16..19, sky in 20..23 — `lm_light`'s word shape.
layout(location = 3) in uint in_light;
// Which texture-array layer: a block texture for a terrain shard, a particle
// sprite for everything else. One array so both share a pipeline.
layout(location = 4) in uint in_layer;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) out flat uint v_layer;

void main() {
    gl_Position = pc.mvp * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_layer = in_layer;
    vec3 lm = lm_light(in_light, pc.light, pc.sky_col, pc.ambient.rgb);
    v_color = vec4(in_color.rgb * lm, in_color.a);
}
