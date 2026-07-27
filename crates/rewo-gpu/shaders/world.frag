#version 450
#extension GL_GOOGLE_include_directive : require
#include "lightmap.glsl"
// Sample the block texture array (SRGB image → linear values), apply the
// baked face shade, then fade toward the fog color with distance so the
// world melts into the sky at the render-distance edge (fog color = sky
// horizon). Alpha-test keeps cutout blocks (plants/glass) honest.

layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

// The push block below is exactly 128 bytes — the guaranteed Vulkan budget —
// so M16's dimension AmbientColor rides in this small per-frame uniform buffer
// instead. `WorldRenderer` ring-buffers it alongside the frames in flight.
layout(set = 0, binding = 1) uniform LightmapExtra {
    vec4 ambient;  // xyz = AmbientColor (RGB24/255), w unused (std140 pad)
    // The ENVIRONMENTAL fog band: x start, y end (M33b). Vanilla's
    // `total_fog_value` is the max of an environmental and a render-distance
    // term, and rain thickens only the former. Disabled by default (start past
    // any real distance), so clear weather is unaffected.
    vec4 env_fog;
} lmx;

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
    vec3 lm = lm_light(v_layer, pc.light, pc.sky_col, lmx.ambient.rgb);
    vec3 rgb = c.rgb * v_color * lm;
    float dist = distance(pc.cam_fog.xyz, v_worldpos);
    // `total_fog_value` — the MAX of the render-distance band (the push
    // block's, which dissolves the chunk edge into the sky) and the
    // environmental one (which rain thickens). Vanilla takes the same max.
    float fog = clamp((dist - pc.cam_fog.w) / max(pc.fog_col.w - pc.cam_fog.w, 1.0), 0.0, 1.0);
    float env = clamp((dist - lmx.env_fog.x) / max(lmx.env_fog.y - lmx.env_fog.x, 1.0), 0.0, 1.0);
    fog = max(fog, env);
    rgb = mix(rgb, pc.fog_col.rgb, fog);
    out_color = vec4(rgb, 1.0);
}
