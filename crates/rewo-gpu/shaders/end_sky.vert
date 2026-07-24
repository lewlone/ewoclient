#version 450
// The End skybox cube (`SkyRenderer.buildEndSky` / `renderEndSky`).
//
// Six quads of a ±100 cube, UV 0..16 (the sampler REPEATs, so `end_sky.png`
// tiles 16× per face), each vertex carrying the constant vanilla colour
// `-14145496` = 0xFF282828. The CPU supplies the colour already **linearized**
// (see `end_sky.rs`) because Rewo's attachment is `R8G8B8A8_SRGB` and re-encodes
// on store — that is a fact about our pipeline; the decompile says nothing about
// vanilla's target colour space, and the equivalence is pinned by `skyshot`.
//
// Drawn in rotation-only sky space: the caller passes `view_proj · T(eye)`, so
// the cube stays centred on the camera exactly as vanilla's translation-
// stripped model-view does.

layout(push_constant) uniform PC {
    mat4 mvp;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec4 in_color;

layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec4 v_color;

void main() {
    gl_Position = pc.mvp * vec4(in_pos, 1.0);
    v_uv = in_uv;
    v_color = in_color;
}
