#version 450

// `rendertype_crumbling.fsh`:
//
//     vec4 color = texture(Sampler0, texCoord0) * vertexColor;
//     if (color.a < 0.1) discard;
//     color = color * ColorModulator;
//     fragColor = apply_fog(color, ...);
//
// `vertexColor` is forced to white by `SheetedDecalTextureGenerator.setColor`
// (it calls `delegate.setColor(-1)` and drops the argument), and
// `ColorModulator` is white for this render type — so both multiplies are the
// identity and the fragment is the texel.
//
// The alpha cut at 0.1 is what makes the crack a *crack*: `destroy_stage_N`
// is mostly transparent, and only the dark lines survive to reach the
// multiply blend.
//
// The fog term is not applied. Vanilla culls this geometry at 32 blocks
// (`distToCenterSqr > 1024.0`), which is inside the render-distance fade at
// every view distance Rewo runs; the environmental band is the one case where
// this could show, and it is recorded as a scoped exclusion rather than
// half-implemented.

layout(set = 0, binding = 0) uniform sampler2DArray tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_stage;

layout(location = 0) out vec4 out_color;

void main() {
    // The array is uploaded UNORM, and the attachment this pass draws into is
    // reopened UNORM, so the texel travels to the blender gamma-encoded —
    // which is where vanilla's `2 * src * dst` multiply happens. See the pass.
    vec4 c = texture(tex, vec3(v_uv, float(v_stage)));
    if (c.a < 0.1) {
        discard;
    }
    out_color = c;
}
