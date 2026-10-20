#version 450
// M285 — `RenderPipelines.ENERGY_SWIRL` (`core/entity` with ALPHA_CUTOUT 0.1,
// EMISSIVE, NO_OVERLAY, NO_CARDINAL_LIGHTING, APPLY_TEXTURE_MATRIX): the
// charged creeper's shell. The texture transform's offset is already in the
// UVs (the CPU adds it; the sheet's sampler repeats), the colour is the
// layer's `0xFF808080`, and EMISSIVE samples no lightmap. Blended
// `ADDITIVE` (ONE, ONE) through the UNORM view, so the numbers are vanilla's
// gamma-space ones; fog is dropped, as the glint's is.

layout(set = 0, binding = 0) uniform sampler2D u_swirl;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 c = texture(u_swirl, v_uv) * v_color;
    if (c.a < 0.1) {
        discard;
    }
    out_color = c;
}
