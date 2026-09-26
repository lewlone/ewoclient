#version 450
#extension GL_GOOGLE_include_directive : require
#include "lightmap.glsl"
// World pass: vertices are already in WORLD space (the mesher emits
// cx*16+lx+corner), so no per-column origin is applied here. The world
// position also rides through to the fragment for distance fog.
//
// The vertex is vanilla's block vertex (see `rewo_mesh::MeshVertex`, 28 B):
// pos f32x3 | uv f32x2 | light u32 | color RGBA8_UNORM. Like vanilla's
// `terrain.vsh` (`vertexColor = Color * sample_lightmap(Sampler2, UV2)`), the
// lightmap is sampled here, per vertex, and the product is what interpolates.

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 cam_fog; // xyz camera pos, w = fog start distance
    vec4 fog_col; // xyz fog color (linear), w = fog end distance
    vec4 light;   // sky factor, block factor, brightness factor, darkness scale
    vec4 sky_col; // xyz sky light color, w = night-vision factor
} pc;

// See world.frag: the dimension AmbientColor rides in this UBO because the
// push block is full.
layout(set = 0, binding = 1) uniform LightmapExtra {
    vec4 ambient;
    vec4 env_fog;
} lmx;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;    // R32G32_SFLOAT (exact)
layout(location = 2) in uint in_light; // layer[0..15] block[16..23] sky[24..31] (smooth units)
layout(location = 3) in vec4 in_color; // R8G8B8A8_UNORM, vanilla's `Color`

layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_layer;
layout(location = 2) out vec3 v_color;
layout(location = 3) out vec3 v_worldpos;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_layer = in_light & 0xFFFFu;
    vec3 lm = lm_sample((in_light >> 16) & 0xFFu, in_light >> 24, pc.light, pc.sky_col, lmx.ambient.rgb);
    v_color = in_color.rgb * lm;
    v_worldpos = in_pos;
}
