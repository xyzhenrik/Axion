// Procedural stand-in for a web page: animated hero, text lines and image cards that scroll.
// Optionally shows a user-provided image instead (tiled vertically, scrolls the same way).

struct Page {
    resolution: vec2f,
    scroll: f32,
    time: f32,
    dark: f32,
    use_image: f32,
    image_size: vec2f,
}

@group(0) @binding(0) var<uniform> page: Page;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var image: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4f,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4f(uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
    return out;
}

fn hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn blob(p: vec2f, c: vec2f, r: f32) -> f32 {
    let d = length(p - c) / r;
    return exp(-d * d);
}

fn rect(p: vec2f, min_p: vec2f, max_p: vec2f, r: f32) -> f32 {
    let c = (min_p + max_p) * 0.5;
    let b = (max_p - min_p) * 0.5;
    let q = abs(p - c) - b + vec2f(r);
    let d = length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
    return clamp(0.5 - d, 0.0, 1.0);
}

fn hue(h: f32) -> vec3f {
    let k = vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0);
    return clamp(abs(fract(vec3f(h) + k) * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4f {
    let res = page.resolution;
    let p = vec2f(in.pos.x, in.pos.y + page.scroll);

    if (page.use_image > 0.5) {
        let tile_h = res.x * page.image_size.y / page.image_size.x;
        let uv = vec2f(in.pos.x / res.x, fract(p.y / tile_h));
        return vec4f(textureSampleLevel(image, samp, uv, 0.0).rgb, 1.0);
    }

    let bg = mix(vec3f(0.965, 0.965, 0.975), vec3f(0.07, 0.07, 0.085), page.dark);
    let ink = mix(vec3f(0.16, 0.17, 0.2), vec3f(0.82, 0.83, 0.86), page.dark);
    let faint = mix(vec3f(0.62, 0.64, 0.68), vec3f(0.4, 0.41, 0.45), page.dark);
    var col = bg;

    // Hero: drifting colour blobs.
    let hero_h = 560.0;
    if (p.y < hero_h) {
        let t = page.time * 0.35;
        var c = vec3f(0.05, 0.06, 0.16);
        c += vec3f(1.0, 0.25, 0.55) * blob(p, vec2f(res.x * (0.3 + 0.15 * sin(t)), 200.0 + 80.0 * cos(t * 1.3)), 320.0);
        c += vec3f(1.0, 0.6, 0.1) * blob(p, vec2f(res.x * (0.7 + 0.1 * cos(t * 0.7)), 300.0 + 60.0 * sin(t)), 280.0);
        c += vec3f(0.1, 0.5, 1.0) * blob(p, vec2f(res.x * (0.5 + 0.25 * sin(t * 0.5)), 120.0), 260.0);
        c += vec3f(0.1, 0.9, 0.7) * blob(p, vec2f(res.x * 0.15, 480.0 + 40.0 * sin(t * 2.0)), 220.0);
        col = c;
        // Hero headline.
        let w = min(res.x - 160.0, 900.0);
        let x0 = (res.x - w) * 0.5;
        col = mix(col, vec3f(1.0), rect(p, vec2f(x0, 300.0), vec2f(x0 + w * 0.62, 352.0), 10.0));
        col = mix(col, vec3f(0.92), rect(p, vec2f(x0, 380.0), vec2f(x0 + w * 0.42, 398.0), 9.0));
        return vec4f(col, 1.0);
    }

    // Article body in a centred column.
    let w = min(res.x - 160.0, 900.0);
    let x0 = (res.x - w) * 0.5;
    let section_h = 760.0;
    let sy = p.y - hero_h;
    let section = floor(sy / section_h);
    let ly = sy - section * section_h;

    // Heading.
    let hw = w * (0.45 + 0.3 * hash(vec2f(section, 1.0)));
    col = mix(col, ink, rect(vec2f(p.x, ly), vec2f(x0, 60.0), vec2f(x0 + hw, 92.0), 6.0));

    // Paragraph lines.
    if (ly > 120.0 && ly < 400.0) {
        let line = floor((ly - 120.0) / 28.0);
        let lw = w * select(0.82 + 0.18 * hash(vec2f(section, line)), 0.5, line == 9.0);
        let top = 120.0 + line * 28.0;
        col = mix(col, faint, rect(vec2f(p.x, ly), vec2f(x0, top + 8.0), vec2f(x0 + lw, top + 20.0), 6.0));
    }

    // A row of three image cards.
    if (ly > 440.0 && ly < 700.0) {
        let gap = 20.0;
        let cw = (w - 2.0 * gap) / 3.0;
        for (var i = 0; i < 3; i++) {
            let cx = x0 + f32(i) * (cw + gap);
            let inside = rect(vec2f(p.x, ly), vec2f(cx, 440.0), vec2f(cx + cw, 700.0), 18.0);
            if (inside > 0.0) {
                let h = hash(vec2f(section, f32(i)));
                let local = (vec2f(p.x - cx, ly - 440.0)) / vec2f(cw, 260.0);
                let g = mix(hue(h), hue(fract(h + 0.25)), local.x * 0.6 + local.y * 0.4);
                // Diagonal stripes make refraction easy to see.
                let stripes = 0.88 + 0.12 * step(0.5, fract((local.x + local.y) * 8.0));
                col = mix(col, g * stripes, inside);
            }
        }
    }
    return vec4f(col, 1.0);
}
