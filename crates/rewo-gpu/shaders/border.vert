#version 450
// The world border wall — `core/rendertype_world_border.vsh`:
//
//     vec3 pos    = Position + ModelOffset;
//     gl_Position = ProjMat * ModelViewMat * vec4(pos, 1.0);
//     texCoord0   = (TextureMat * vec4(UV0, 0.0, 1.0)).xy;
//
// Two of those three pieces are folded away here.
//
// **`ModelOffset` is gone.** Vanilla's is `(lastMinX - cameraX, -cameraY,
// lastMinZ - cameraZ)` against a camera-relative model-view, which nets out to
// plain world space. Rewo's `view_proj` already carries the camera (the M33
// lesson: the relative form draws every wall around the world origin), so the
// CPU emits world-space positions and this is just the MVP.
//
// **`TextureMat` is a translation and nothing else** — `new Matrix4f()
// .translation(offset, offset, 0.0F)` — so the 4x4 collapses to adding the
// scroll to the UV. The scroll is the one wall-clock quantity in this feature:
// `(Util.getMillis() % 3000L) / 3000.0F`.

layout(push_constant) uniform PC {
    mat4 mvp;
    // `ColorModulator` — the `BorderStatus` tint in rgb, `state.alpha` in a.
    vec4 color;
    // The scroll, applied to both u and v as vanilla's matrix does.
    vec2 tex_offset;
    vec2 _pad;
} pc;

layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;

layout(location = 0) out vec2 v_uv;

void main() {
    gl_Position = pc.mvp * vec4(in_pos, 1.0);
    v_uv = in_uv + pc.tex_offset;
}
