#version 450
// Velvet chrome (M52b step 3) — `draw_iw_shell`'s six layers, evaluated
// analytically from one rounded-box SDF instead of six Skia draws.
//
// Layer order and every constant is transcribed from
// `ewo-jni/src/hud.rs::draw_iw_shell_driven`. Do not "tidy" the numbers.
//
//   0. music bloom     rose 0.42*energy, blur 10+16*energy, grown by 2+6*pulse
//   1. drop shadow     black 0.55, offset (0, +6), sigma 8
//   2. outer wine ring rect+1, radius+1, 1px stroke, WINE 0.55
//   3. fill            WINE 0.50
//   4. inset wine ring rect-1, radius-1, 1px stroke, WINE 0.25
//   5. top highlight   rect+0.5, radius-0.5, 1px stroke, PEARL 0.10,
//                      CLIPPED to the top 2px
//   6. pearl border    rect, (1 + pulse*0.8)px stroke,
//                      PEARL (0.18 + level*0.32 + pulse*0.28)
//
// A Gaussian mask blur over a rounded rect does NOT need a blur pass: the
// coverage is a smoothstep over the SDF. That is what lets a plate with a
// shadow and a bloom cost one fragment evaluation rather than three
// ping-pongs, and it is most of why this holds 120fps with many widgets up.

layout(location = 0) in vec2 v_px;      // fragment position in shell-local px
layout(location = 1) in vec4 v_rect;    // (cx, cy, halfw, halfh)
layout(location = 2) in vec4 v_params;  // (radius, level, pulse, alpha)

layout(location = 0) out vec4 out_color;

// The palette is DATA, not a constant. EwoClient's HUD is getting a visual
// overhaul, so baking Velvet's specific colours into a shader would guarantee
// a shader edit for what should be a table edit. What stays in here is the
// STRUCTURE -- six SDF layers in Skia's draw order -- because that is what a
// palette change does not touch.
layout(set = 0, binding = 0) uniform Style {
    vec4 bloom;      // rgb + alpha scale
    vec4 shadow;
    vec4 outer_ring;
    vec4 fill;
    vec4 inset_ring;
    vec4 top_highlight;
    vec4 border;
} style;

float sdRoundBox(vec2 p, vec2 b, float r) {
    vec2 q = abs(p) - b + vec2(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0))) - r;
}

// Analytic 1px-feather coverage of the region d <= 0.
float cov(float d) {
    return clamp(0.5 - d, 0.0, 1.0);
}

// A 1px-wide stroke centred on the contour d == 0.
float stroke(float d, float w) {
    return clamp(w * 0.5 + 0.5 - abs(d), 0.0, 1.0);
}

// Coverage of a Gaussian-blurred rounded rect. `smoothstep` over ~2 sigma is
// the standard closed-form stand-in for the true integral and is well within
// a byte of it for the sigmas here.
float blurred(float d, float sigma) {
    return 1.0 - smoothstep(-sigma, sigma * 1.2, d);
}

// Source-over, done in the SAME order Skia draws so the result matches layer
// for layer rather than only in total.
vec4 over(vec4 dst, vec3 c, float a) {
    float na = a + dst.a * (1.0 - a);
    vec3 nc = na > 0.0 ? (c * a + dst.rgb * dst.a * (1.0 - a)) / na : vec3(0.0);
    return vec4(nc, na);
}

void main() {
    vec2 p = v_px - v_rect.xy;
    vec2 half_ = v_rect.zw;
    float radius = v_params.x;
    float level = v_params.y;
    float pulse = v_params.z;

    float d = sdRoundBox(p, half_, radius);
    vec4 acc = vec4(0.0);

    // 0. Music bloom, under everything, so the plate reads as glowing rather
    //    than as a ring stuck to it.
    float energy = level * 0.30 + pulse * 0.70;
    if (energy > 0.01) {
        float grow = 2.0 + 6.0 * pulse;
        float db = sdRoundBox(p, half_ + vec2(grow), radius + grow);
        acc = over(acc, style.bloom.rgb, blurred(db, 10.0 + 16.0 * energy) * style.bloom.a * energy);
    }

    // 1. Drop shadow: the same rrect, dropped 6px, sigma 8.
    float ds = sdRoundBox(p - vec2(0.0, 6.0), half_, radius);
    acc = over(acc, style.shadow.rgb, blurred(ds, 8.0) * style.shadow.a);

    // 2. Outer wine ring, 1px outside.
    acc = over(acc, style.outer_ring.rgb,
        stroke(sdRoundBox(p, half_ + vec2(1.0), radius + 1.0), 1.0) * style.outer_ring.a);

    // 3. Fill.
    acc = over(acc, style.fill.rgb, cov(d) * style.fill.a);

    // 4. Inset wine ring, 1px inside.
    acc = over(acc, style.inset_ring.rgb,
        stroke(sdRoundBox(p, half_ - vec2(1.0), max(radius - 1.0, 0.0)), 1.0)
        * style.inset_ring.a);

    // 5. Top pearl highlight — clipped to the top 2px. The clip is why this
    //    reads as a lip catching light rather than as a second full ring.
    float top_clip = step(p.y, -half_.y + 2.0);
    acc = over(acc, style.top_highlight.rgb,
        stroke(sdRoundBox(p, half_ - vec2(0.5), max(radius - 0.5, 0.0)), 1.0)
        * style.top_highlight.a * top_clip);

    // 6. Pearl border, outermost, music-reactive.
    // The music terms stay structural: `border.a` is the RESTING alpha and the
    // two drive gains scale from it, so a palette change moves the resting
    // value without flattening the reaction.
    acc = over(acc, style.border.rgb,
        stroke(d, 1.0 + pulse * 0.8) * (style.border.a + level * 0.32 + pulse * 0.28));

    float a = acc.a * v_params.w;
    if (a < 0.002) {
        discard;
    }
    out_color = vec4(acc.rgb, a);
}
