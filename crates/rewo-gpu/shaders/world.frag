#version 450
// Sample the block texture array (SRGB image → linear values) and apply the
// baked face shade. Alpha-test keeps future cutout blocks honest.

layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_layer;
layout(location = 2) in float v_shade;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_tex, vec3(v_uv, float(v_layer)));
    if (c.a < 0.5) {
        discard;
    }
    out_color = vec4(c.rgb * v_shade, 1.0);
}
