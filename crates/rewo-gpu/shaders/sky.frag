#version 450
// Gradient sky. Reconstruct the pixel's world-space view ray from the
// inverse view_proj, then blend horizon→zenith by how far up it looks.
// Colors arrive pre-linearized (the SRGB attachment encodes on store);
// the horizon color is shared with the terrain's distance fog so the
// world fades seamlessly into the sky at the render-distance edge.

layout(location = 0) in vec2 v_clip;
layout(location = 0) out vec4 out_color;

layout(push_constant) uniform PC {
    mat4 inv_view_proj;
    vec4 horizon; // linear rgb
    vec4 zenith;  // linear rgb
} pc;

void main() {
    // Two points down the ray at different depths → direction (reversed-Z:
    // 1.0 is the near plane, smaller is farther; 0.5 is a stable far point).
    vec4 a = pc.inv_view_proj * vec4(v_clip, 1.0, 1.0);
    vec4 b = pc.inv_view_proj * vec4(v_clip, 0.5, 1.0);
    vec3 dir = normalize(b.xyz / b.w - a.xyz / a.w);

    float up = dir.y;
    // Horizon at up=0, zenith by ~27°; slight darken looking below horizon.
    float t = smoothstep(0.0, 0.45, up);
    vec3 col = mix(pc.horizon.rgb, pc.zenith.rgb, t);
    col = mix(col, pc.horizon.rgb * 0.82, smoothstep(0.0, -0.35, up));
    out_color = vec4(col, 1.0);
}
