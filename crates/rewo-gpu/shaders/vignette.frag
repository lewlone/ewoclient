#version 450
// M282/M283 — the camera overlays (`Hud.extractCameraOverlays`): one image
// stretched over a screen-pixel rect (the scissor clips to it), times the
// blit's colour. The vignette's pipeline multiplies the frame by one minus it
// (`RenderPipelines.VIGNETTE`: ZERO, ONE_MINUS_SRC_COLOR); the others blend
// by alpha (`GUI_TEXTURED`). Drawn through the frame's UNORM view: vanilla's
// numbers are gamma-encoded.

layout(set = 0, binding = 0) uniform sampler2D u_tex;

layout(push_constant) uniform Push {
    vec4 rect;
    vec4 color;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    vec2 uv = (gl_FragCoord.xy - pc.rect.xy) / (pc.rect.zw - pc.rect.xy);
    out_color = texture(u_tex, uv) * pc.color;
}
