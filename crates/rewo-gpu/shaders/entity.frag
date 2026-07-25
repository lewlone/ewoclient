#version 450
// Sample the font atlas (SRGB image → linear texels). Solid geometry
// (capsules, tag backgrounds) points its UVs at the atlas's patched white
// texel, so one pipeline family covers glyphs and fills. Vertex colors
// arrive pre-linearized from the CPU (render discipline: sRGB constants
// never hit an SRGB attachment raw).
//
// M21 — the damage flash. Vanilla's `entity.fsh` is:
//
//     color *= faceVertexColor * ColorModulator;
//     color.rgb = mix(overlayColor.rgb, color.rgb, overlayColor.a);
//     color *= lightMapColor;
//
// so the overlay lands on `texture * vertexColor` and the lightmap multiplies
// *after* it — which is why `v_light_hurt.rgb` carries the light separately
// instead of the CPU folding it into `v_color`. `overlayColor` is a texel of
// `OverlayTexture`: the red row (v=3) is 0xB3FF0000, i.e. rgb (1,0,0) with
// a = 179/255, and the no-overlay texel (u=0, v=10) is white with a = 1.0 —
// which makes the mix a no-op, exactly as vanilla intends.

layout(set = 0, binding = 0) uniform sampler2D u_font;

layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec4 v_color;
// rgb = per-channel world light, a = the hurt flag (0 or 1).
layout(location = 2) in vec4 v_light_hurt;

layout(location = 0) out vec4 out_color;

// `OverlayTexture`'s red row alpha, 0xB3 = 179.
const float HURT_OVERLAY_ALPHA = 179.0 / 255.0;

// Rewo works in linear light; vanilla mixes in its gamma-encoded texture
// space. `mix` is not invariant under the transfer function, so mixing
// linearly would give a visibly different flash. Convert, mix, convert back.
vec3 to_srgb(vec3 c) {
    return mix(c * 12.92,
               1.055 * pow(max(c, vec3(0.0)), vec3(1.0 / 2.4)) - 0.055,
               greaterThan(c, vec3(0.0031308)));
}

vec3 to_linear(vec3 c) {
    return mix(c / 12.92,
               pow((c + 0.055) / 1.055, vec3(2.4)),
               greaterThan(c, vec3(0.04045)));
}

void main() {
    vec4 t = texture(u_font, v_uv);
    float a = t.a * v_color.a;
    if (a < 0.004) {
        discard;
    }
    vec3 c = t.rgb * v_color.rgb;
    if (v_light_hurt.a > 0.5) {
        // mix(overlay.rgb, color.rgb, overlay.a) against the red row's texel.
        vec3 s = to_srgb(c);
        s = mix(vec3(1.0, 0.0, 0.0), s, HURT_OVERLAY_ALPHA);
        c = to_linear(s);
    }
    c *= v_light_hurt.rgb;
    out_color = vec4(c, a);
}
