#version 450
// The Velvet glyph atlas is R8_UNORM COVERAGE, so the mask is in `.r` --
// vanilla's font atlas is RGBA and puts it in `.a`. Sampling `.a` here would
// return 1.0 everywhere for a single-channel image and paint solid rectangles
// over the HUD, which is a strikingly obvious failure and therefore worth a
// comment so nobody "fixes" it back.
layout(set = 0, binding = 0) uniform sampler2D u_glyphs;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    float cov = texture(u_glyphs, v_uv).r * v_color.a;
    if (cov < 0.002) {
        discard;
    }
    // Premultiplied-by-coverage colour, straight alpha out -- the pass blends
    // SRC_ALPHA/ONE_MINUS_SRC_ALPHA exactly like text.frag.
    out_color = vec4(v_color.rgb, cov);
}
