#version 450
// GUI items (M34). The texture times the face's diffuse, with the same
// alpha cutout every item path uses — an item sprite is mostly transparent and
// a blended edge would fringe against the hotbar behind it.

layout(set = 0, binding = 0) uniform sampler2D u_atlas;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in float v_shade;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_atlas, v_uv);
    if (c.a < 0.1) {
        discard;
    }
    out_color = vec4(c.rgb * v_shade, c.a);
}
