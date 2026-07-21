#version 450
// Frame-time strip chart: one bar per ring sample, oldest -> newest left to
// right, colored by frame budget. Velvet palette. No text (M0 has no font).

layout(push_constant) uniform Params {
    vec2 origin;    // chart top-left, framebuffer px
    vec2 size;      // chart size, px
    float scale_ms; // frame time mapped to full chart height
    uint count;     // ring capacity
    uint head;      // index of the OLDEST sample in the ring
} pc;

layout(set = 0, binding = 0, std430) readonly buffer Samples {
    float samples_ms[];
};

layout(location = 0) out vec4 out_color;

// The attachment is SRGB: shader output is LINEAR and the hardware encodes
// on store. Palette constants below are authored in sRGB (straight from the
// Velvet hex values), so convert — writing them raw double-encodes and
// washes every color out. This rule applies to the whole renderer.
vec3 srgb_to_linear(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c));
}

vec3 bar_color(float ms) {
    if (ms <= 4.17) return srgb_to_linear(vec3(0.788, 0.647, 0.831)); // lavender: >= 240 fps
    if (ms <= 8.33) return srgb_to_linear(vec3(0.898, 0.722, 0.773)); // rose:     >= 120 fps
    if (ms <= 16.7) return srgb_to_linear(vec3(0.910, 0.831, 0.659)); // champagne:>= 60 fps
    return srgb_to_linear(vec3(0.788, 0.416, 0.478));                 // ember: hitch
}

void main() {
    vec2 rel = gl_FragCoord.xy - pc.origin;
    if (rel.x < 0.0 || rel.y < 0.0 || rel.x >= pc.size.x || rel.y >= pc.size.y) {
        discard;
    }
    float n = float(pc.count);
    uint i = uint(clamp(rel.x / (pc.size.x / n), 0.0, n - 1.0));
    float ms = samples_ms[(pc.head + i) % pc.count];
    float from_bottom = pc.size.y - rel.y;

    // Budget gridlines: 60 fps and 120 fps.
    float g60 = (16.7 / pc.scale_ms) * pc.size.y;
    float g120 = (8.33 / pc.scale_ms) * pc.size.y;
    if (abs(from_bottom - g60) < 1.0 || abs(from_bottom - g120) < 1.0) {
        out_color = vec4(srgb_to_linear(vec3(0.957, 0.910, 0.918)), 0.35); // pearl hairline
        return;
    }

    float h = clamp(ms / pc.scale_ms, 0.0, 1.0) * pc.size.y;
    if (from_bottom <= h) {
        out_color = vec4(bar_color(ms), 0.9);
        return;
    }
    out_color = vec4(srgb_to_linear(vec3(0.04, 0.0, 0.024)), 0.78); // dark-wine chart backdrop
}
