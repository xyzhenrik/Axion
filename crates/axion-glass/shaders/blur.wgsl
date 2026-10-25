// One direction of a separable Gaussian blur. Run twice (horizontal, then vertical) into
// quarter-resolution targets; linear filtering does the downsampling.

struct BlurParams {
    // Distance between taps, in UV units of the full-resolution backdrop.
    step_uv: vec2f,
    _pad: vec2f,
}

@group(0) @binding(0) var<uniform> blur: BlurParams;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var src: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

const TAPS: i32 = 6;
const SIGMA: f32 = 3.0;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    var acc = vec3f(0.0);
    var weight_sum = 0.0;
    for (var i = -TAPS; i <= TAPS; i++) {
        let x = f32(i);
        let w = exp(-x * x / (2.0 * SIGMA * SIGMA));
        acc += textureSampleLevel(src, samp, in.uv + blur.step_uv * x, 0.0).rgb * w;
        weight_sum += w;
    }
    return vec4f(acc / weight_sum, 1.0);
}
