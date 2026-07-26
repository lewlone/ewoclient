#version 450
// `core/rendertype_end_portal.vsh` — the end portal / gateway.
//
//     gl_Position = ProjMat * ModelViewMat * vec4(Position, 1.0);
//     texProj0    = projection_from_position(gl_Position);
//
// The vertex format is POSITION ONLY. There is no UV and no colour, because
// the fragment shader does not sample in model space at all — it samples in
// SCREEN space, through the projective coordinate built here. That is why the
// portal's mesh UVs were never used, and why a portal's starfield slides as
// the camera moves rather than being painted on the quad.
//
// `projection_from_position` (shaders/include/projection.glsl):
//
//     vec4 projection = position * 0.5;
//     projection.xy   = vec2(projection.x + projection.w, projection.y + projection.w);
//     projection.zw   = position.zw;
//
// i.e. the standard clip → [0,1] projective remap on x and y, with z and w
// passed through untouched so `textureProj` divides by the original w.

layout(push_constant) uniform PC {
    mat4 mvp;
    // `GameTime` — vanilla's is the world clock scaled into 0..1 per 24000
    // ticks. Supplied by the CPU so a headless render is reproducible.
    float game_time;
    // 15 for a portal, 16 for a gateway. A shader *define* in vanilla; a
    // uniform here, so one pipeline serves both.
    int layers;
} pc;

layout(location = 0) in vec3 in_pos;

layout(location = 0) out vec4 v_tex_proj;

void main() {
    gl_Position = pc.mvp * vec4(in_pos, 1.0);

    vec4 projection = gl_Position * 0.5;
    projection.xy = vec2(projection.x + projection.w, projection.y + projection.w);
    projection.zw = gl_Position.zw;
    v_tex_proj = projection;
}
