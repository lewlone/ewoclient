#version 450
// Sunrise/sunset colour = per-vertex colour × tint. Vanilla blends this with
// TRANSLUCENT (src=SRC_ALPHA dst=ONE_MINUS_SRC_ALPHA): the alpha-1 centre
// paints the warm colour, fading to nothing at the alpha-0 ring. Fully-
// transparent fragments are discarded exactly as vanilla's `position_color.fsh`
// does (alpha == 0.0).

layout(push_constant) uniform PC {
    mat4 mvp;
    vec4 tint;
} pc;

layout(location = 0) in vec4 v_color;
layout(location = 0) out vec4 out_color;

void main() {
    out_color = v_color * pc.tint;
    if (out_color.a == 0.0) {
        discard;
    }
}
