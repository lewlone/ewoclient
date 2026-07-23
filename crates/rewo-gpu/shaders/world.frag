#version 450
#extension GL_GOOGLE_include_directive : require
#include "lightmap.glsl"
// Sample the block texture array (SRGB image → linear values), apply the
// baked face shade, then fade toward the fog color with distance so the
// world melts into the sky at the render-distance edge (fog color = sky
// horizon). Alpha-test keeps cutout blocks (plants/glass) honest.

layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 cam_fog; // xyz camera pos, w = fog start
    vec4 fog_col; // xyz fog color (linear), w = fog end
    vec4 light;   // sky factor, block factor, brightness factor, darkness scale
    vec4 sky_col; // xyz sky light color (white by day, blue at night), w = night-vision factor
} pc;   // 128 bytes — the guaranteed push-constant budget is now full,
        // so anything further has to move into a UBO.

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_layer;
layout(location = 2) in vec3 v_color;
layout(location = 3) in vec3 v_worldpos;

layout(location = 0) out vec4 out_color;

void main() {
    // The low 16 bits are the texture layer; the upper bits carry the two
    // light levels (`rewo_mesh::pack_layer`).
    vec4 c = texture(u_tex, vec3(v_uv, float(v_layer & 0xFFFFu)));
    if (c.a < 0.5) {
        discard;
    }
    vec3 lm = lm_light(v_layer, pc.light, pc.sky_col);
    vec3 rgb = c.rgb * v_color * lm;
    float dist = distance(pc.cam_fog.xyz, v_worldpos);
    float fog = clamp((dist - pc.cam_fog.w) / max(pc.fog_col.w - pc.cam_fog.w, 1.0), 0.0, 1.0);
    rgb = mix(rgb, pc.fog_col.rgb, fog);
    out_color = vec4(rgb, 1.0);
}
