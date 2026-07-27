#version 450
// GUI items — the icons in hotbar and inventory slots (M34).
//
// The whole transform happens on the CPU. An item in a slot is static for the
// frame: one `display.gui` transform, no articulation, and vanilla's diffuse is
// per-face rather than per-pixel (the quads carry flat normals), so there is
// nothing per-vertex left for the GPU to do but project.
//
// Positions arrive in **screen pixels, top-left origin**, matching the HUD
// pass. The third component is a depth in the same pixel scale, which exists
// only so the faces of a 3D block item sort against each other — it is not
// related to the world's depth buffer, and this pass owns its own.

layout(push_constant) uniform PC {
    vec2 screen;
    // Half-extent of the depth range, in the same pixel units the positions
    // use. A block item is 0.625 of a block across at 16 px per block, so its
    // corners reach about ±9 px; 64 is comfortably clear of that and keeps the
    // mapping linear and obvious.
    float depth_scale;
    float _pad;
} pc;

layout(location = 0) in vec3 in_pos;   // x, y in pixels; z in the same units
layout(location = 1) in vec2 in_uv;
// The face's resolved diffuse, already through
// `minecraft_mix_light_separate` on the CPU.
layout(location = 2) in float in_shade;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out float v_shade;

void main() {
    vec2 ndc = in_pos.xy / pc.screen * 2.0 - 1.0;
    // Reversed-Z like the rest of Rewo: nearer is greater, so negate.
    float z = 0.5 - in_pos.z / (2.0 * pc.depth_scale);
    gl_Position = vec4(ndc, z, 1.0);
    v_uv = in_uv;
    v_shade = in_shade;
}
