// Instanced rounded rectangles: solid fills (anti-aliased) and soft shadows.

struct Viewport {
    size: vec2f,
    _pad: vec2f,
}

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct Instance {
    // x, y, width, height in physical pixels.
    @location(0) rect: vec4f,
    @location(1) color: vec4f,
    // corner radius, softness (0 = crisp edge, > 0 = shadow blur radius), ring width (0 = filled),
    // knockout (1 = a shadow trimmed under its own shape).
    @location(2) params: vec4f,
}

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) local: vec2f,
    @location(1) half_size: vec2f,
    @location(2) color: vec4f,
    @location(3) params: vec4f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2f(f32(vi & 1u), f32((vi >> 1u) & 1u));
    // Grow the quad so soft edges and anti-aliasing aren't clipped.
    let pad = inst.params.y + 1.0;
    let p = inst.rect.xy - vec2f(pad) + corner * (inst.rect.zw + vec2f(2.0 * pad));
    var out: VsOut;
    out.pos = vec4f(p / viewport.size * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    out.half_size = inst.rect.zw * 0.5;
    out.local = p - (inst.rect.xy + out.half_size);
    out.color = inst.color;
    out.params = inst.params;
    return out;
}

fn sd_round_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let rr = clamp(r, 0.0, min(b.x, b.y));
    let q = abs(p) - b + vec2f(rr);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let d = sd_round_box(in.local, in.half_size, in.params.x);
    let softness = in.params.y;
    var coverage: f32;
    if (softness > 0.0) {
        coverage = 1.0 - smoothstep(-softness * 0.5, softness, d);
        if (in.params.w > 0.5) {
            // Not under the card itself: a translucent card must not look darker for its shadow.
            coverage = coverage * clamp(d + 0.5, 0.0, 1.0);
        }
    } else {
        coverage = clamp(0.5 - d, 0.0, 1.0);
        let ring = in.params.z;
        if (ring > 0.0) {
            // Only the band between the edge and `ring` px inside it.
            coverage = coverage * clamp(d + ring + 0.5, 0.0, 1.0);
        }
    }
    return vec4f(in.color.rgb, in.color.a * coverage);
}
