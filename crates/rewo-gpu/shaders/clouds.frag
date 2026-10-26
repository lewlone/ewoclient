#version 450
// `core/rendertype_clouds.fsh`, transcribed. The whole shader is
//
//     vec4 color = vertexColor;
//     color.a *= 1.0 - linear_fog_value(vertexDistance, 0, FogCloudsEnd);
//     fragColor = color;
//
// with the fog fade folded into the vertex stage here, since it is a per-vertex
// quantity interpolated either way and clouds are flat-shaded per face.

//
// M435 — `v_color` is vanilla's number (the face shade times the encoded
// cloud colour). Compiled with GAMMA_OUT for the gamma-space scope, where the
// translucent blend runs on encoded numbers as vanilla's does; without a
// UNORM twin the plain build decodes it once for the sRGB attachment.

layout(location = 0) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
#ifdef GAMMA_OUT
    out_color = v_color;
#else
    vec3 c = clamp(v_color.rgb, 0.0, 1.0);
    out_color = vec4(mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)), v_color.a);
#endif
}
