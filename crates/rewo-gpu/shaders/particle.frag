#version 450
// `assets/minecraft/shaders/core/particle.fsh` (M35):
//
//     vec4 color = texture(Sampler0, texCoord0) * vertexColor * ColorModulator;
//     if (color.a < 0.1) discard;
//     fragColor = apply_fog(...);
//
// `ColorModulator` is white here. The **discard** is load-bearing rather than
// an optimisation: the particle atlas is mostly transparent, and an alpha-blend
// without the cutoff leaves every quad's empty corners as a faint square halo.
//
// Vanilla's fog term is omitted for the same reason every other Rewo pass
// omits it — fog is applied in the world pass, and a second application would
// double it.

layout(set = 0, binding = 0) uniform sampler2D u_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(u_tex, v_uv) * v_color;
    if (color.a < 0.1) {
        discard;
    }
    out_color = color;
}
