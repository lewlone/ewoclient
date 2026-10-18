#version 450
// The overworld sky (M209): vanilla's sky disc, evaluated per pixel.
//
// `SkyRenderer.buildSkyDisc(16)` is a triangle fan: a centre (0, 16, 0) and
// nine ring vertices `(512 cos i, 16, 512 sin i)` for i = -180..180 step 45 —
// an octagon of circumradius 512, centred on the camera (the model-view has no
// translation). `sky.vsh` computes each vertex's spherical and cylindrical fog
// distance and the rasterizer interpolates them across the fan, so a point of
// the disc a fraction `s` of the way from the centre to its edge chord gets
// `16 + (512.25 - 16) s` and `16 + (512 - 16) s`. `sky.fsh` then applies
// `apply_fog(skyColor, sph, cyl, 0, skyEnd, skyEnd, skyEnd, fogColor)`.
// Where the disc does not cover a pixel (at or below the horizon, or past its
// rim), what shows is the main target's clear colour: the fog colour
// (`LevelRenderer`: `clearColorAndDepthTextures(.., fogColor, ..)`).
//
// `renderDarkDisc` (eye below the level's horizon height, not underwater) is
// the mirrored fan built at y = -16, drawn translated up 12 (so at -4) in
// black — but its fog distances come from the untranslated positions.
//
// The terrain fades to the same fog colour, so the far edge of the world
// meets the sky without a seam. (Before M209 this was an invented gradient
// whose horizon was the biome SKY colour while the terrain faded to the FOG
// colour — a hard line wherever terrain ended below the horizon.)
// Colours arrive linear and mix in linear, as the world pass's fog does.

layout(location = 0) in vec2 v_clip;
layout(location = 0) out vec4 out_color;

layout(push_constant) uniform PC {
    mat4 inv_view_proj;
    vec4 sky;    // linear rgb: the sky colour (biome sky x time-of-day tint)
    vec4 fog;    // linear rgb: the fog colour, also the clear colour
    vec4 params; // x = skyEnd, y = 1.0 when the dark disc draws
} pc;

// The octagon's apothem, 512 cos(22.5 deg).
const float APOTHEM = 473.0294;
const float RING_SPH = 512.24994; // length((512, 16))

// The fraction of the way from the fan's centre to its edge chord: the
// largest projection on the eight apothem normals (at 22.5 + 45k degrees),
// over the apothem. Above 1 the point is past the rim.
float fan_fraction(vec2 p) {
    float m = -1e30;
    for (int k = 0; k < 8; k++) {
        float a = radians(22.5 + 45.0 * float(k));
        m = max(m, dot(p, vec2(cos(a), sin(a))));
    }
    return m / APOTHEM;
}

// `linear_fog_value` as fog.glsl writes it.
float linear_fog(float v, float start, float end) {
    if (v <= start) {
        return 0.0;
    } else if (v >= end) {
        return 1.0;
    }
    return (v - start) / (end - start);
}

// `total_fog_value(sph, cyl, 0, skyEnd, skyEnd, skyEnd)` at fan fraction s.
float disc_fog(float s, float sky_end) {
    float sph = 16.0 + (RING_SPH - 16.0) * s;
    float cyl = 16.0 + (512.0 - 16.0) * s;
    return max(linear_fog(sph, 0.0, sky_end), linear_fog(cyl, sky_end, sky_end));
}

void main() {
    // Two points down the ray at different depths -> direction (reversed-Z:
    // 1.0 is the near plane, smaller is farther; 0.5 is a stable far point).
    vec4 a = pc.inv_view_proj * vec4(v_clip, 1.0, 1.0);
    vec4 b = pc.inv_view_proj * vec4(v_clip, 0.5, 1.0);
    vec3 dir = normalize(b.xyz / b.w - a.xyz / a.w);
    float sky_end = pc.params.x;

    vec3 col = pc.fog.rgb;
    if (dir.y > 0.0) {
        float s = fan_fraction(dir.xz * (16.0 / dir.y));
        if (s <= 1.0) {
            col = mix(pc.sky.rgb, pc.fog.rgb, disc_fog(s, sky_end));
        }
    }
    if (pc.params.y > 0.5 && dir.y < 0.0) {
        float s = fan_fraction(dir.xz * (-4.0 / dir.y));
        if (s <= 1.0) {
            col = mix(vec3(0.0), pc.fog.rgb, disc_fog(s, sky_end));
        }
    }
    out_color = vec4(col, 1.0);
}
