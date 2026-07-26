#version 450
// `core/particle.fsh`:
//
//     vec4 color = texture(Sampler0, texCoord0) * vertexColor * ColorModulator;
//     if (color.a < 0.1) discard;
//     fragColor = apply_fog(...);
//
// `ColorModulator` is white for weather. The **discard** is what gives rain its
// streaks: `rain.png` is mostly transparent, and without the cutoff the whole
// column would read as a translucent sheet.
//
// Vanilla's fog term is omitted for the same reason the end portal's is: Rewo
// applies fog in its own world pass, and a second application would double it.

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
