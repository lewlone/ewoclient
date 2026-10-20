#version 450
// M282 — `RenderPipelines.VIGNETTE`: `misc/vignette.png` stretched over the
// GUI, times the blit's colour; the pipeline's blend (ZERO,
// ONE_MINUS_SRC_COLOR) multiplies the frame by one minus it. Drawn through the
// frame's UNORM view: vanilla's numbers are gamma-encoded.

layout(set = 0, binding = 0) uniform sampler2D u_tex;

layout(push_constant) uniform Push {
    vec2 size;
    vec2 pad;
    vec4 color;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = texture(u_tex, gl_FragCoord.xy / pc.size) * vec4(pc.color.rgb, 1.0);
}
