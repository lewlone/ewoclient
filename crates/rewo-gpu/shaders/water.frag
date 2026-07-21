#version 450
// Translucent (water) pass: same inputs as world.frag, but the texture's
// alpha rides through to the blender (water_still ships alpha 180) instead
// of the opaque path's alpha-test. Color = texture × baked shade/light.

layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_layer;
layout(location = 2) in vec3 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_tex, vec3(v_uv, float(v_layer)));
    out_color = vec4(c.rgb * v_color, c.a);
}
