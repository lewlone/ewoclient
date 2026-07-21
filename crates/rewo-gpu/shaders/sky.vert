#version 450
// Fullscreen-triangle sky: covers the framebuffer, passes the clip-space
// position to the fragment so it can reconstruct a per-pixel view ray
// through the SAME view_proj the terrain uses (the negative-Y viewport
// applies to both, so the rays line up with the drawn world).

layout(location = 0) out vec2 v_clip;

void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    vec2 clip = p * 2.0 - 1.0;
    v_clip = clip;
    gl_Position = vec4(clip, 0.0, 1.0);
}
