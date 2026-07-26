#version 450
// `core/rendertype_clouds.vsh`, transcribed.
//
// The mesh is not vertices. The CPU writes **three bytes per quad** — cell x,
// cell z, and a direction-plus-flags byte — and this shader expands each into
// four corners from a hardcoded 24-entry table. That is why `CloudRenderer`
// rebuilds only when the camera crosses a cell boundary: the buffer is tiny and
// position-independent, and the per-frame offset rides in a uniform instead.
//
// Rewo stores those three bytes as three `int`s in a storage buffer rather than
// vanilla's `isamplerBuffer` of `R8I` texels. That is a representation change,
// not a behavioural one: the values written are exactly what vanilla's signed
// bytes sign-extend to, which is what the bit arithmetic below already assumes
// (a `dirAndFlags` with bit 7 set is *negative* as a Java byte, and vanilla
// relies on `& FLAG_EXTRA_X` recovering it anyway).

layout(std140, set = 0, binding = 0) uniform CloudInfo {
    vec4 cloud_color;
    // `-xInCell, relativeBottomY, -zInCell` — the sub-cell offset, so the cell
    // grid itself stays integral and only this moves per frame.
    vec4 cloud_offset;
    // Always (12, 4, 12): `CELL_SIZE_IN_BLOCKS` and the 4-block cloud slab.
    vec4 cell_size;
} info;

layout(std430, set = 0, binding = 1) readonly buffer CloudFaces {
    int faces[];
} cloud_faces;

layout(push_constant) uniform PC {
    mat4 mvp;
    // `FogCloudsEnd`. Clouds fade out on their own linear ramp from 0, not on
    // the world pass's fog band.
    float fog_clouds_end;
} pc;

const int FLAG_MASK_DIR = 7;
const int FLAG_INSIDE_FACE = 1 << 4;
const int FLAG_USE_TOP_COLOR = 1 << 5;
const int FLAG_EXTRA_Z = 1 << 6;
const int FLAG_EXTRA_X = 1 << 7;

layout(location = 0) out vec4 v_color;

const vec3 vertices[24] = vec3[](
    // Bottom face
    vec3(1, 0, 0), vec3(1, 0, 1), vec3(0, 0, 1), vec3(0, 0, 0),
    // Top face
    vec3(0, 1, 0), vec3(0, 1, 1), vec3(1, 1, 1), vec3(1, 1, 0),
    // North face
    vec3(0, 0, 0), vec3(0, 1, 0), vec3(1, 1, 0), vec3(1, 0, 0),
    // South face
    vec3(1, 0, 1), vec3(1, 1, 1), vec3(0, 1, 1), vec3(0, 0, 1),
    // West face
    vec3(0, 0, 1), vec3(0, 1, 1), vec3(0, 1, 0), vec3(0, 0, 0),
    // East face
    vec3(1, 0, 0), vec3(1, 1, 0), vec3(1, 1, 1), vec3(1, 0, 1)
);

// Clouds carry no texture at all — six fixed shades, indexed by direction.
const vec4 face_colors[6] = vec4[](
    vec4(0.7, 0.7, 0.7, 1.0), // Bottom
    vec4(1.0, 1.0, 1.0, 1.0), // Top
    vec4(0.8, 0.8, 0.8, 1.0), // North
    vec4(0.8, 0.8, 0.8, 1.0), // South
    vec4(0.9, 0.9, 0.9, 1.0), // West
    vec4(0.9, 0.9, 0.9, 1.0)  // East
);

float linear_fog_value(float vertex_distance, float fog_start, float fog_end) {
    if (vertex_distance <= fog_start) {
        return 0.0;
    } else if (vertex_distance >= fog_end) {
        return 1.0;
    }
    return (vertex_distance - fog_start) / (fog_end - fog_start);
}

// Vanilla draws `PrimitiveTopology.QUADS` through a shared sequential index
// buffer. Vulkan has no quad topology, so the six triangle vertices per quad
// are mapped back onto its four corners here — 0,1,2 then 0,2,3, the same
// winding vanilla's sequential buffer produces. This is the only structural
// difference from `rendertype_clouds.vsh`.
const int QUAD_CORNER[6] = int[](0, 1, 2, 0, 2, 3);

void main() {
    int quad_vertex = QUAD_CORNER[gl_VertexIndex % 6];
    int index = (gl_VertexIndex / 6) * 3;

    int cell_x = cloud_faces.faces[index];
    int cell_z = cloud_faces.faces[index + 1];
    int dir_and_flags = cloud_faces.faces[index + 2];
    int direction = dir_and_flags & FLAG_MASK_DIR;
    bool is_inside_face = (dir_and_flags & FLAG_INSIDE_FACE) == FLAG_INSIDE_FACE;
    bool use_top_color = (dir_and_flags & FLAG_USE_TOP_COLOR) == FLAG_USE_TOP_COLOR;
    // The low bit of each cell coordinate rides in the flags byte, because the
    // coordinate itself was shifted right by one to fit in a signed byte.
    cell_x = (cell_x << 1) | ((dir_and_flags & FLAG_EXTRA_X) >> 7);
    cell_z = (cell_z << 1) | ((dir_and_flags & FLAG_EXTRA_Z) >> 6);

    // An interior face reverses its winding so it faces the camera inside the
    // cloud — the same quad, wound the other way.
    vec3 face_vertex = vertices[(direction * 4) + (is_inside_face ? 3 - quad_vertex : quad_vertex)];
    vec3 pos = (face_vertex * info.cell_size.xyz)
             + (vec3(cell_x, 0, cell_z) * info.cell_size.xyz)
             + info.cloud_offset.xyz;
    gl_Position = pc.mvp * vec4(pos, 1.0);

    vec4 color = (use_top_color ? face_colors[1] : face_colors[direction]) * info.cloud_color;
    // `fog_spherical_distance` is `length(pos)`, and `pos` is already relative
    // to the camera.
    color.a *= 1.0 - linear_fog_value(length(pos), 0.0, pc.fog_clouds_end);
    v_color = color;
}
