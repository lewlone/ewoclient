#version 450
// `core/rendertype_clouds.fsh`, transcribed. The whole shader is
//
//     vec4 color = vertexColor;
//     color.a *= 1.0 - linear_fog_value(vertexDistance, 0, FogCloudsEnd);
//     fragColor = color;
//
// with the fog fade folded into the vertex stage here, since it is a per-vertex
// quantity interpolated either way and clouds are flat-shaded per face.

layout(location = 0) in vec4 v_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = v_color;
}
