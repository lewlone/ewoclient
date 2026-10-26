#version 450
// Translucent (water) pass: same inputs as world.frag, but the texture's
// alpha rides through to the blender (water_still ships alpha 180) instead
// of the opaque path's alpha-test. Distance fog fades the rgb toward the
// sky horizon; the surface's own alpha is preserved.

layout(set = 0, binding = 0) uniform sampler2DArray u_tex;
// M197: the fluid flow sprites' 32 px array. A layer with the high bit set
// (`rewo_mesh::FLOW_LAYER_BIT`) indexes it — a flowing fluid top.
layout(set = 0, binding = 2) uniform sampler2DArray u_flow;

// Same `LightmapExtra` UBO as world.frag — translucent and opaque geometry MUST
// resolve the identical lightmap, ambient included.
layout(set = 0, binding = 1) uniform LightmapExtra {
    vec4 ambient;  // xyz = AmbientColor (RGB24/255), w unused (std140 pad)
    // The ENVIRONMENTAL fog band: x start, y end (M33b). Vanilla's
    // `total_fog_value` is the max of an environmental and a render-distance
    // term, and rain thickens only the former. Disabled by default (start past
    // any real distance), so clear weather is unaffected.
    vec4 env_fog;
} lmx;

layout(push_constant) uniform PC {
    mat4 view_proj;
    vec4 cam_fog; // xyz camera pos, w = fog start
    vec4 fog_col; // xyz fog color (linear), w = fog end
    vec4 light;   // sky factor, block factor, brightness factor, darkness scale
    vec4 sky_col; // xyz sky light color, w = night-vision factor
} pc;

layout(location = 0) in vec2 v_uv;
layout(location = 1) flat in uint v_layer;
layout(location = 2) in vec3 v_color;
layout(location = 3) in vec3 v_worldpos;

layout(location = 0) out vec4 out_color;

// M421 — `fog.glsl`'s `linear_fog_value`: piecewise, with no floor on the
// band's width (the 1-block floor Rewo divided by halved lava's 0.25..1 band;
// an empty band, start == end, is a step).
float linear_fog_value(float d, float s, float e) {
    if (d <= s) {
        return 0.0;
    } else if (d >= e) {
        return 1.0;
    }
    return (d - s) / (e - s);
}

void main() {
    // `v_layer` is flat, so the branch is uniform over the primitive (and its
    // quads): the implicit-LOD samples below stay well defined.
    vec4 c;
    if ((v_layer & 0x8000u) != 0u) {
        c = texture(u_flow, vec3(v_uv, float(v_layer & 0x7FFFu)));
    } else {
        c = texture(u_tex, vec3(v_uv, float(v_layer)));
    }
    // `v_color` already carries the lightmap (world.vert, per vertex).
    vec3 rgb = c.rgb * v_color;
    vec3 rel = v_worldpos - pc.cam_fog.xyz;
    // M206: vanilla's fog.glsl measures the environmental term with
    // `fog_spherical_distance` and the render-distance one with
    // `fog_cylindrical_distance`, max(length(xz), |y|).
    float dist = length(rel);
    float cyl = max(length(rel.xz), abs(rel.y));
    // `total_fog_value` — the MAX of the render-distance band (the push
    // block's, which dissolves the chunk edge into the sky) and the
    // environmental one (which rain thickens). Vanilla takes the same max.
    float fog = linear_fog_value(cyl, pc.cam_fog.w, pc.fog_col.w);
    float env = linear_fog_value(dist, lmx.env_fog.x, lmx.env_fog.y);
    fog = max(fog, env);
    rgb = mix(rgb, pc.fog_col.rgb, fog);
    out_color = vec4(rgb, c.a);
}
