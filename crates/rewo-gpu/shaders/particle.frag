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

// A single array holding the block textures and the particle sprites, so a
// block-break shard and a flame reach the same sampler.
layout(set = 0, binding = 0) uniform sampler2DArray u_tex;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
layout(location = 2) in flat uint v_layer;

layout(location = 0) out vec4 out_color;

void main() {
    vec4 color = texture(u_tex, vec3(v_uv, float(v_layer))) * v_color;
    if (color.a < 0.1) {
        discard;
    }
    out_color = color;
}
