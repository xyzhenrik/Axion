// Average colours of a texture region, rendered into a grid of `cells` × `cells` pixels (each
// the average of its part of the region). One pixel picks the chrome's light/dark appearance;
// a 32 × 32 grid is the page's palette, from which its dominant colour tints the window.

struct Probe {
    // u, v, width, height of the region in 0–1.
    region: vec4f,
    // x: cells per side.
    cells: vec4f,
}

@group(0) @binding(0) var<uniform> probe: Probe;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var tex: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
}

const GRID: i32 = 12;

@fragment
fn fs_main(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let cell = floor(position.xy);
    var sum = vec3f(0.0);
    for (var y = 0; y < GRID; y++) {
        for (var x = 0; x < GRID; x++) {
            let t = (cell + (vec2f(f32(x), f32(y)) + 0.5) / f32(GRID)) / probe.cells.x;
            sum += textureSampleLevel(tex, samp, probe.region.xy + t * probe.region.zw, 0.0).rgb;
        }
    }
    return vec4f(sum / f32(GRID * GRID), 1.0);
}
