#version 450
// The locator bar's dot is `blitSprite(..., color)`, i.e. vanilla's
// `texture * vertexColor` — and vanilla evaluates that product in GAMMA
// space: its GUI textures carry no sRGB view, so `texture()` hands the shader
// the raw byte/255 and every number downstream is gamma-encoded. This is
// M50's rule (the enchantment glint), and the same trap: multiplying in
// linear is a different quantity, and the difference is largest exactly where
// one factor is small.
//
// So the atlas is uploaded UNORM (raw bytes), the tint arrives gamma-encoded,
// the multiply happens here, and the result is decoded to linear because the
// colour attachment is sRGB and re-encodes on store. With a white tint that
// round-trip is the identity, so an untinted quad matches `hud.frag` exactly.

layout(set = 0, binding = 0) uniform sampler2D u_atlas;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 0) out vec4 out_color;

vec3 srgb_to_linear(vec3 c) {
    bvec3 cutoff = lessThanEqual(c, vec3(0.04045));
    vec3 low = c / 12.92;
    vec3 high = pow((c + 0.055) / 1.055, vec3(2.4));
    return mix(high, low, vec3(cutoff));
}

void main() {
    vec4 c = texture(u_atlas, v_uv);
    if (c.a < 0.004) {
        discard;
    }
    vec3 g = c.rgb * v_color.rgb;
    out_color = vec4(srgb_to_linear(g), c.a * v_color.a);
}
