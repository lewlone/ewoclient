#version 450
// M278 — one triangle covering the screen (vanilla's `core/screenquad`). The
// fragment shaders address their input by `gl_FragCoord`, so no varyings.

void main() {
    vec2 p = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
