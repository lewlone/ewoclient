#version 450
// M278 — vanilla's `post/entity_sobel.fsh`, verbatim but for addressing the
// input by `gl_FragCoord` (input and output are the same size, so
// `texCoord` is the fragment's centre over the size).

layout(set = 0, binding = 0) uniform sampler2D u_in;

layout(push_constant) uniform Push {
    vec2 in_size;
    vec2 blur_dir;
} pc;

layout(location = 0) out vec4 out_color;

void main() {
    vec2 one = 1.0 / pc.in_size;
    vec2 uv = gl_FragCoord.xy / pc.in_size;
    vec4 center = texture(u_in, uv);
    vec4 left = texture(u_in, uv - vec2(one.x, 0.0));
    vec4 right = texture(u_in, uv + vec2(one.x, 0.0));
    vec4 up = texture(u_in, uv - vec2(0.0, one.y));
    vec4 down = texture(u_in, uv + vec2(0.0, one.y));
    float leftDiff = abs(center.a - left.a);
    float rightDiff = abs(center.a - right.a);
    float upDiff = abs(center.a - up.a);
    float downDiff = abs(center.a - down.a);
    float total = clamp(leftDiff + rightDiff + upDiff + downDiff, 0.0, 1.0);
    vec3 outColor = center.rgb * center.a + left.rgb * left.a + right.rgb * right.a
        + up.rgb * up.a + down.rgb * down.a;
    out_color = vec4(outColor * 0.2, total);
}
