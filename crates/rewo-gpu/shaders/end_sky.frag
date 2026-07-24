#version 450
// `core/position_tex_color`: texel × vertex colour, no lighting, no fog term
// of our own. The texture is uploaded as R8G8B8A8_SRGB so the sample is already
// linear; the vertex colour arrives linearized on the CPU side.
//
// The pipeline uses BlendFunction.TRANSLUCENT (END_SKY's ColorTargetState).
// `end_sky.png` is fully opaque, so the blend resolves to a plain overwrite —
// but the blend is specified rather than assumed so a non-opaque resource pack
// texture would composite the way vanilla's does.

layout(set = 0, binding = 0) uniform sampler2D u_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(u_tex, v_uv) * v_color;
}
