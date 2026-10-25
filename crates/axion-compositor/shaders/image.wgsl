// One textured rectangle with rounded corners and opacity (page frames, thumbnails, blits).

struct Draw {
    // x, y, width, height in physical pixels.
    rect: vec4f,
    // Texture region to show: u, v, width, height in 0–1.
    uv: vec4f,
    viewport: vec2f,
    radius: f32,
    opacity: f32,
}

@group(0) @binding(0) var<uniform> draw: Draw;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let corner = vec2f(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let p = draw.rect.xy + corner * draw.rect.zw;
    var out: VsOut;
    out.pos = vec4f(p / draw.viewport * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    out.uv = corner;
    return out;
}

fn texture_uv(corner: vec2f) -> vec2f {
    return draw.uv.xy + corner * draw.uv.zw;
}

fn sd_round_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let rr = clamp(r, 0.0, min(b.x, b.y));
    let q = abs(p) - b + vec2f(rr);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let color = textureSampleLevel(tex, samp, texture_uv(in.uv), 0.0);
    var alpha = color.a * draw.opacity;
    if (draw.radius > 0.0) {
        let half_size = draw.rect.zw * 0.5;
        let d = sd_round_box((in.uv - 0.5) * draw.rect.zw, half_size, draw.radius);
        alpha *= clamp(0.5 - d, 0.0, 1.0);
    }
    return vec4f(color.rgb, alpha);
}
