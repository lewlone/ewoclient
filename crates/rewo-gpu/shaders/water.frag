#version 450
// Translucent (water) pass: same inputs as world.frag, but the texture's
// alpha rides through to the blender (water_still ships alpha 180) instead
// of the opaque path's alpha-test. Distance fog fades the rgb toward the
// sky horizon; the surface's own alpha is preserved.

layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 cam_fog; // xyz camera pos, w = fog start
    vec4 fog_col; // xyz fog color (linear), w = fog end
} pc;

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_layer;
layout(location = 2) in vec3 v_color;
layout(location = 3) in vec3 v_worldpos;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_tex, vec3(v_uv, float(v_layer)));
    vec3 rgb = c.rgb * v_color;
    float dist = distance(pc.cam_fog.xyz, v_worldpos);
    float fog = clamp((dist - pc.cam_fog.w) / max(pc.fog_col.w - pc.cam_fog.w, 1.0), 0.0, 1.0);
    rgb = mix(rgb, pc.fog_col.rgb, fog);
    out_color = vec4(rgb, c.a);
}
