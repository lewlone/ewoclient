#version 450
// Entity pass (capsules + nametags): CPU-built world-space triangle soup —
// positions, shading, and billboard orientation are all baked on the CPU
// per frame (entity counts are tiny), so this stays a plain transform.

layout(push_constant) uniform PC {
    mat4 view_proj;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;
// rgb = per-channel world light, a = the hurt flag (M21). Kept out of
// `in_color` because the damage overlay has to be mixed in *before* the
// lightmap multiply, matching vanilla's `entity.fsh` order.
layout(location = 3) in vec4 in_light_hurt;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;
layout(location = 2) out vec4 v_light_hurt;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_color = in_color;
    v_light_hurt = in_light_hurt;
}
