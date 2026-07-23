#version 450
// Sun / moon texel × tint. The atlas is an sRGB texture so the sampler returns
// linear; the SRGB attachment re-encodes on store. Blended with vanilla's
// CELESTIAL OVERLAY function (color src=SRC_ALPHA dst=ONE): the disc's alpha
// masks it and its colour adds as a glow over the sky. Fully-transparent texels
// are discarded exactly as vanilla's `position_tex.fsh` does (alpha == 0.0), so
// they never touch the RGBA attachment.

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 tint;
} pc;

layout(set = 0, binding = 0) uniform sampler2D tex;

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(tex, v_uv) * pc.tint;
    if (out_color.a == 0.0) {
        discard;
    }
}
