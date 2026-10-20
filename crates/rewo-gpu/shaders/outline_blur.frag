#version 450
// M278 — vanilla's `post/entity_outline_box_blur.fsh` (radius 2, sampled
// bilinearly), run once along x and once along y:
//
//     for (float a = -radius + 0.5; a <= radius; a += 2.0)
//         blurred += texture(InSampler, texCoord + sampleStep * a);
//     blurred += texture(InSampler, texCoord + sampleStep * radius) / 2.0;
//     fragColor = vec4((blurred / (radius + 0.5)).rgb, blurred.a);

layout(set = 0, binding = 0) uniform sampler2D u_in;

layout(push_constant) uniform Push {
    vec2 in_size;
    vec2 blur_dir;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    vec2 uv = gl_FragCoord.xy / pc.in_size;
    vec2 step_ = pc.blur_dir / pc.in_size;
    float radius = 2.0;
    vec4 blurred = vec4(0.0);
    for (float a = -radius + 0.5; a <= radius; a += 2.0) {
        blurred += texture(u_in, uv + step_ * a);
    }
    blurred += texture(u_in, uv + step_ * radius) / 2.0;
    out_color = vec4((blurred / (radius + 0.5)).rgb, blurred.a);
}
