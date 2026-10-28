// Liquid Glass composite pass.
//
// Draws the backdrop and, on top of it, every glass shape: refraction from the SDF normal,
// chromatic dispersion, frost (blurred backdrop), tint, specular rim and drop shadow.
// All distances are in physical pixels; the origin is the top-left corner.

struct Params {
    tint: vec4f,
    resolution: vec2f,
    light_dir: vec2f,
    refraction: f32,
    bezel: f32,
    dispersion: f32,
    merge_k: f32,
    rim: f32,
    shadow: f32,
    frost: f32,
    shape_count: u32,
    // 1 when some shape has its own tint strength or glow (see `scene_tinted`).
    own_tints: u32,
    // Shapes from this index on merge with wide_k (bridges, sorted last by the renderer).
    wide_from: u32,
    wide_k: f32,
}

struct Shape {
    center: vec2f,
    half_size: vec2f,
    stretch: vec2f,
    radius: f32,
    angle: f32,
    // -1: the pass's tint strength (params.tint.a); JOIN_ONLY: none of its own.
    tint_opacity: f32,
    // Brightening inside the shape (0–1, a pressed lens).
    glow: f32,
    // How much of the pass's drop shadow the shape casts (0-1).
    shadow: f32,
    pad: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> shapes: array<Shape>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var backdrop: texture_2d<f32>;
@group(0) @binding(4) var blurred: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the screen.
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn sd_round_box(p: vec2f, b: vec2f, r: f32) -> f32 {
    let rr = clamp(r, 0.0, min(b.x, b.y));
    let q = abs(p) - b + vec2f(rr);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - rr;
}

fn rotate(v: vec2f, a: f32) -> vec2f {
    let c = cos(a);
    let s = sin(a);
    return vec2f(c * v.x - s * v.y, s * v.x + c * v.y);
}

fn shape_sdf(s: Shape, p: vec2f) -> f32 {
    // Squash/stretch along `angle` without rotating the shape itself.
    var d = rotate(p - s.center, -s.angle);
    d = d / s.stretch;
    d = rotate(d, s.angle);
    return sd_round_box(d, s.half_size, s.radius) * min(s.stretch.x, s.stretch.y);
}

// Polynomial smooth minimum: the "liquid" union of two fields.
fn smin(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 0.001) {
        return min(a, b);
    }
    let h = max(k - abs(a - b), 0.0) / k;
    return min(a, b) - h * h * k * 0.25;
}

// Conservative lower bound of `shape_sdf`, from a bounding circle. Cheap to evaluate.
fn shape_bound(s: Shape, p: vec2f) -> f32 {
    let max_s = max(s.stretch.x, s.stretch.y);
    let min_s = min(s.stretch.x, s.stretch.y);
    let reach = length(s.half_size) * max_s;
    return (length(p - s.center) - reach) * min_s / max_s;
}

fn scene(p: vec2f) -> f32 {
    var d = 1e5;
    for (var i = 0u; i < params.shape_count; i++) {
        let s = shapes[i];
        let k = select(params.merge_k, params.wide_k, i >= params.wide_from);
        // smin(d, x) == d whenever x >= d + k, so far-away shapes can be skipped.
        if (shape_bound(s, p) >= d + k) {
            continue;
        }
        d = smin(d, shape_sdf(s, p), k);
    }
    return d;
}

// `Shape::tint_opacity` of a shape without a tint of its own.
const JOIN_ONLY: f32 = -2.0;

// How far into its soft edge a glowing shape brightens (px, inside the edge) …
const GLOW_EDGE: f32 = 6.0;
// … and how much clearer it gets per unit of glow (less tint).
const GLOW_CLEARING: f32 = 3.0;

// `scene` plus the tint strength and glow at `p`: each shape's own strength, blended across
// the smooth union the same way the shapes flow together; the glow of the brightest shape
// `p` lies in. Only evaluated once per pixel.
fn scene_tinted(p: vec2f) -> vec3f {
    var d = 1e5;
    var a = params.tint.a;
    var g = 0.0;
    for (var i = 0u; i < params.shape_count; i++) {
        let s = shapes[i];
        let k = select(params.merge_k, params.wide_k, i >= params.wide_from);
        // A glowing shape lies inside others, so it is never skipped (its glow counts).
        if (shape_bound(s, p) >= d + k && s.glow <= 0.0) {
            continue;
        }
        let x = shape_sdf(s, p);
        // A shape that only joins others leaves the tint to them.
        if (s.tint_opacity > JOIN_ONLY + 0.5) {
            let own = select(params.tint.a, s.tint_opacity, s.tint_opacity >= 0.0);
            // Weight of what was there before: 1 where it is much nearer, 0 where this shape is.
            let keep = clamp(0.5 + 0.5 * (x - d) / max(k, 0.001), 0.0, 1.0);
            a = mix(own, a, keep);
        }
        g = max(g, s.glow * clamp(0.5 - x / GLOW_EDGE, 0.0, 1.0));
        d = smin(d, x, k);
    }
    return vec3f(d, a, g);
}

fn sample3(tex: texture_2d<f32>, uv_r: vec2f, uv_g: vec2f, uv_b: vec2f) -> vec3f {
    return vec3f(
        textureSampleLevel(tex, samp, uv_r, 0.0).r,
        textureSampleLevel(tex, samp, uv_g, 0.0).g,
        textureSampleLevel(tex, samp, uv_b, 0.0).b,
    );
}

const SHADOW_OFFSET: vec2f = vec2f(0.0, 8.0);
const SHADOW_REACH: f32 = 28.0;

// The drop shadow's strength (0-1) at `q`: the strongest of what the shapes that cast one
// throw there. Buttons cast none (`Shape::shadow` = 0).
fn shadow_at(q: vec2f) -> f32 {
    var m = 0.0;
    for (var i = 0u; i < params.shape_count; i++) {
        let s = shapes[i];
        if (s.shadow <= 0.0 || shape_bound(s, q) >= SHADOW_REACH) {
            continue;
        }
        let ds = shape_sdf(s, q);
        m = max(m, s.shadow * (1.0 - smoothstep(-6.0, SHADOW_REACH, ds)));
    }
    return m;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let p = in.pos.xy;
    let base = textureSampleLevel(backdrop, samp, in.uv, 0.0).rgb;
    var field = vec3f(0.0, params.tint.a, 0.0);
    if (params.own_tints != 0u) {
        field = scene_tinted(p);
    } else {
        field.x = scene(p);
    }
    let d = field.x;

    // Far from any glass: plain backdrop.
    if (d > SHADOW_REACH + SHADOW_OFFSET.y) {
        return vec4f(base, 1.0);
    }

    // Soft drop shadow, slightly offset downwards. Only visible outside the glass (and in
    // the anti-aliased edge), so it isn't evaluated deeper inside.
    var outside = base;
    if (d > -1.0) {
        let shadow = params.shadow * shadow_at(p - SHADOW_OFFSET);
        outside = base * (1.0 - 0.5 * shadow);
    }
    if (d >= 1.0) {
        return vec4f(outside, 1.0);
    }

    // Surface normal from the SDF gradient (central differences). In the flat interior
    // the normal has no effect (no bend, no rim), so skip the four extra evaluations.
    let depth = max(-d, 0.0);
    var n = vec2f(0.0, -1.0);
    if (depth < params.bezel + 3.0) {
        let e = 1.0;
        let grad = vec2f(
            scene(p + vec2f(e, 0.0)) - scene(p - vec2f(e, 0.0)),
            scene(p + vec2f(0.0, e)) - scene(p - vec2f(0.0, e)),
        );
        n = grad / max(length(grad), 1e-5);
    }

    // Circular bezel: flat in the middle, increasingly steep towards the rim.
    let edge = 1.0 - clamp(depth / params.bezel, 0.0, 1.0);
    let bend = 1.0 - sqrt(max(1.0 - edge * edge, 0.0));

    // Light bends towards the thicker centre, so the rim shows content pulled from inside.
    let offset = -n * bend * params.refraction;
    let disp = params.dispersion * bend;
    let inv_res = 1.0 / params.resolution;
    let uv_r = (p + offset * (1.0 + disp)) * inv_res;
    let uv_g = (p + offset) * inv_res;
    let uv_b = (p + offset * (1.0 - disp)) * inv_res;

    let sharp = sample3(backdrop, uv_r, uv_g, uv_b);
    let soft = sample3(blurred, uv_r, uv_g, uv_b);
    var glass = mix(sharp, soft, params.frost);
    // A pressed lens is clearer and lighter, like glass swelling into a lens that gathers
    // light: less tint shows more of what is behind it.
    glass = mix(glass, params.tint.rgb, field.y * max(1.0 - field.z * GLOW_CLEARING, 0.0));
    if (field.z > 0.0) {
        glass = mix(glass, vec3f(1.0), field.z);
    }

    // Specular: a thin bright rim all around, strongest where the edge faces the light,
    // plus a weaker reflection on the opposite side and a broad glow across the bezel.
    let l = normalize(params.light_dir);
    let facing = dot(n, l);
    let spec = pow(max(facing, 0.0), 2.0) + 0.45 * pow(max(-facing, 0.0), 2.0);
    let rim_line = 1.0 - smoothstep(0.0, 2.0, depth);
    let highlight = params.rim * (rim_line * (0.3 + 0.7 * spec) + bend * spec * 0.35);
    glass = glass + vec3f(highlight);

    // Anti-aliased edge.
    let alpha = clamp(0.5 - d, 0.0, 1.0);
    return vec4f(mix(outside, glass, alpha), 1.0);
}
