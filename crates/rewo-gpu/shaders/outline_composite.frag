#version 450
// M278 — `RenderTarget.blitAndBlendToTexture`: the finished outline over the
// frame, alpha-blended (`BlendFunction.ENTITY_OUTLINE_BLIT`: SRC_ALPHA,
// ONE_MINUS_SRC_ALPHA on colour). Drawn through the frame's UNORM view, so
// the blend works on gamma-encoded numbers, as vanilla's does.

layout(set = 0, binding = 0) uniform sampler2D u_in;

layout(push_constant) uniform Push {
    vec2 in_size;
    vec2 blur_dir;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(u_in, gl_FragCoord.xy / pc.in_size);
}
