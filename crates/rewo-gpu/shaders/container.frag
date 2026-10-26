#version 450
// Texture times vertex colour, both authored in sRGB.
//
// The attachment is an SRGB format, which encodes on store, so the colour
// constants have to be linearised here — the same discipline every other Rewo
// UI pass follows. The texture is already linear (it is sampled from an
// UNORM_SRGB image), so only the vertex colour is converted.

layout(set = 0, binding = 0) uniform sampler2D u_atlas;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

vec3 srgb_to_linear(vec3 c) {
    return mix(c / 12.92,
               pow((c + 0.055) / 1.055, vec3(2.4)),
               step(vec3(0.04045), c));
}

// For the gamma-space variant (`-DGAMMA_OUT`): the result goes to a UNORM view
// of the attachment, where no transfer function is applied on store and the
// fixed-function blend sees vanilla's gamma-encoded numbers.
vec3 linear_to_srgb(vec3 c) {
    return mix(c * 12.92,
               1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055,
               step(vec3(0.0031308), c));
}

void main() {
    // A NEGATIVE u means untextured: the quad is a solid fill of its vertex
    // colour (M93q). Vanilla draws these through a separate pipeline —
    // `GuiGraphics.fill` has no sampler at all — and modelling it as a mode
    // here keeps fills ORDERED with the sprites around them, which matters
    // because the loom's grey backing must sit under its pattern and above the
    // button chrome. A second pass could not interleave them.
    //
    // The sentinel is safe because every real UV is a normalised atlas
    // coordinate in [0, 1]: no legitimate blit can produce a negative one.
    vec4 t = v_uv.x < 0.0 ? vec4(1.0) : texture(u_atlas, v_uv);
    vec3 rgb = t.rgb * srgb_to_linear(v_color.rgb);
#ifdef GAMMA_OUT
    rgb = linear_to_srgb(rgb);
#endif
    out_color = vec4(rgb, t.a * v_color.a);
}
