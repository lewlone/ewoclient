#version 450
// `core/rendertype_end_portal.fsh`, transcribed.
//
//     vec3 color = textureProj(Sampler0, texProj0).rgb * COLORS[0];
//     for (int i = 0; i < PORTAL_LAYERS; i++)
//         color += textureProj(Sampler1, texProj0 * end_portal_layer(float(i + 1))).rgb * COLORS[i];
//     fragColor = vec4(color, 1.0);
//
// Sampler0 is `environment/end_sky.png`, Sampler1 is
// `entity/end_portal/end_portal.png`. `PORTAL_LAYERS` is a shader define: **15
// for a portal and 16 for a gateway** — the gateway really does get one more
// layer, and it is the only difference between the two pipelines.
//
// Two things about the loop are easy to get subtly wrong. The base layer uses
// `COLORS[0]` and so does the FIRST loop iteration (`i = 0`), so that colour is
// applied twice — to different samplers, at different scales. And the layer
// number passed to `end_portal_layer` is `i + 1`, so it runs 1..PORTAL_LAYERS
// while the colour index runs 0..PORTAL_LAYERS-1; they are off by one from each
// other on purpose.
//
// The result is opaque (`vec4(color, 1.0)`) and additive across layers, which
// is why a portal glows rather than compositing — there is no blend state doing
// this, the accumulation is in the shader.
//
// Vanilla's fog term is omitted: Rewo applies fog in its own world pass, and
// adding a second one here would double it.

layout(set = 0, binding = 0) uniform sampler2D u_sky;    // Sampler0
layout(set = 0, binding = 1) uniform sampler2D u_portal; // Sampler1

layout(push_constant) uniform PC {
    mat4 mvp;
    float game_time;
    int layers;
} pc;

layout(location = 0) in vec4 v_tex_proj;

layout(location = 0) out vec4 out_color;

// The sixteen constants, verbatim.
const vec3 COLORS[16] = vec3[](
    vec3(0.022087, 0.098399, 0.110818),
    vec3(0.011892, 0.095924, 0.089485),
    vec3(0.027636, 0.101689, 0.100326),
    vec3(0.046564, 0.109883, 0.114838),
    vec3(0.064901, 0.117696, 0.097189),
    vec3(0.063761, 0.086895, 0.123646),
    vec3(0.084817, 0.111994, 0.166380),
    vec3(0.097489, 0.154120, 0.091064),
    vec3(0.106152, 0.131144, 0.195191),
    vec3(0.097721, 0.110188, 0.187229),
    vec3(0.133516, 0.138278, 0.148582),
    vec3(0.070006, 0.243332, 0.235792),
    vec3(0.196766, 0.142899, 0.214696),
    vec3(0.047281, 0.315338, 0.321970),
    vec3(0.204675, 0.390010, 0.302066),
    vec3(0.080955, 0.314821, 0.661491)
);

// These are LITERAL copies of vanilla's constructors, in the same order.
//
// GLSL's `mat4(...)` fills COLUMN by column, and vanilla's source is GLSL, so
// column 0 of `translate` is `(1, 0, 0, 17/layer)` — the "translation" value
// sits at `m[0][3]`, not `m[3][0]`. It reaches the coordinate because the
// sampling below is `texProj0 * matrix`, a **row-vector** multiply, and
// `v * M` is `transpose(M) * v`.
//
// Rewriting these by assigning elements to the slots they "look like" they
// belong in puts every translate in the wrong place while still producing a
// plausible swirl. Copying them verbatim is the only safe transcription.
const mat4 SCALE_TRANSLATE = mat4(
    0.5, 0.0, 0.0, 0.25,
    0.0, 0.5, 0.0, 0.25,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0
);

mat2 mat2_rotate_z(float radians) {
    return mat2(cos(radians), -sin(radians), sin(radians), cos(radians));
}

mat4 end_portal_layer(float layer) {
    mat4 translate = mat4(
        1.0, 0.0, 0.0, 17.0 / layer,
        0.0, 1.0, 0.0, (2.0 + layer / 1.5) * (pc.game_time * 1.5),
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0
    );

    mat2 rotate = mat2_rotate_z(radians((layer * layer * 4321.0 + layer * 9.0) * 2.0));
    mat2 scale = mat2((4.5 - layer / 4.0) * 2.0);

    return mat4(scale * rotate) * translate * SCALE_TRANSLATE;
}

// GLSL's `textureProj(s, p)` divides p.xy by p.w — spelled out because the
// projective divide is the whole mechanism here.
vec4 tex_proj(sampler2D s, vec4 p) {
    return texture(s, p.xy / p.w);
}

void main() {
    vec3 color = tex_proj(u_sky, v_tex_proj).rgb * COLORS[0];
    for (int i = 0; i < pc.layers; i++) {
        color += tex_proj(u_portal, v_tex_proj * end_portal_layer(float(i + 1))).rgb
               * COLORS[i];
    }
    out_color = vec4(color, 1.0);
}
