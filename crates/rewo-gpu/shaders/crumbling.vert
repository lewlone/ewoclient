#version 450

// `rendertype_crumbling.vsh` — the plainest vertex shader in the game.
// `gl_Position = ProjMat * ModelViewMat * vec4(Position, 1.0)`, and the UV
// passed straight through. The UV it receives is not the model's: the CPU
// regenerated it from the vertex position (see rewo-mesh's `crumbling`).

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in uint in_stage;

layout(push_constant) uniform Push {
    mat4 view_proj;
} pc;

layout(location = 0) out vec2 v_uv;
layout(location = 1) flat out uint v_stage;

void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_stage = in_stage;
}
