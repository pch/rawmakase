// Preview finishing after the develop stage, on the developed pixels still on the
// device: sharpening, then vignettes and grain, the clipping overlay, the monitor
// profile and 8-bit encoding into the texture the viewport draws. Ports of
// `quality::sharpen_with_radius`, `effects::spatial_finish_scaled`, `Rendered::rgb8`
// and `Rendered::histogram`; the CPU versions are the reference.
struct Params {
    // Developed buffer size, and the rectangle of it that is shown.
    width: u32, height: u32, crop_x: u32, crop_y: u32,
    crop_w: u32, crop_h: u32, radius: u32, sharpen: u32,
    amount: f32, threshold: f32, clipping: u32, lut_size: u32,
    // Output pixel of the buffer's first pixel, and the whole output's size.
    origin_x: u32, origin_y: u32, full_w: u32, full_h: u32,
    // `effects::GrainField`.
    scale: f32, grain: f32, grain_cell: f32, grain_coarse: f32,
    // `effects::PostCropVignette`; a zero amount has no vignette.
    grain_seed: u32, vignette: f32, vignette_style: u32, vignette_highlights: f32,
    vignette_scale_x: f32, vignette_scale_y: f32, vignette_power: f32, vignette_midpoint: f32,
    vignette_feather: f32, lens_vignette: f32, lens_vignette_midpoint: f32, effects: u32,
    count: u32, halo: f32, dark: f32, pad2: u32,
    grain_fine: f32, grain_measured: u32, pad3: u32, pad4: u32,
    // `effects::FilmGrainField`; a zero amount has no film grain.
    film_amount: f32, film_radius: f32, film_sigma: f32, film_cell: f32,
    film_max_radius: f32, film_norm: f32, film_seed: u32,
    // Film Grain's Color, and its red, green and blue dye layers as the mono one above.
    film_color: f32, film_red_radius: f32, film_red_cell: f32, film_red_max_radius: f32,
    film_red_norm: f32, film_red_seed: u32, film_green_radius: f32, film_green_cell: f32,
    film_green_max_radius: f32, film_green_norm: f32, film_green_seed: u32,
    film_blue_radius: f32, film_blue_cell: f32, film_blue_max_radius: f32,
    film_blue_norm: f32, film_blue_seed: u32,
    // Film Grain's clumping: its coarse noise, as a layer, and its strength.
    film_clumping: f32, film_clump_radius: f32, film_clump_cell: f32, film_clump_max_radius: f32,
    film_clump_norm: f32, film_clump_seed: u32, film_clump_sigma: f32, pad5: u32, pad6: u32,
};
@group(0) @binding(0) var<storage, read_write> pixels: array<f32>;
@group(0) @binding(1) var<storage, read_write> scratch: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<f32>;
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(4) var<storage, read> lut: array<f32>;
@group(0) @binding(5) var<storage, read_write> histogram: array<atomic<u32>>;
@group(0) @binding(6) var shown: texture_storage_2d<rgba8unorm, write>;

fn rgb(i: u32) -> vec3<f32> {
    return vec3(pixels[3u * i], pixels[3u * i + 1u], pixels[3u * i + 2u]);
}
fn luminance(v: vec3<f32>) -> f32 {
    return 0.2126 * v.r + 0.7152 * v.g + 0.0722 * v.b;
}
@compute @workgroup_size(16, 16)
fn blur_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    var value = 0.0;
    for (var k = 0u; k <= 2u * p.radius; k++) {
        let x = u32(clamp(i32(id.x) + i32(k) - i32(p.radius), 0, i32(p.width) - 1));
        value += luminance(rgb(id.y * p.width + x)) * weights[k];
    }
    scratch[id.y * p.width + id.x] = value;
}
// `sharpening::Sharpener::delta` for a non-negative `amount` (strength already
// applied): the original operator clips at 0.08 (halo 0); the measured one shapes the
// high-pass, dark halos at `dark` of light ones.
fn sharpen_delta(d: f32, amount: f32, threshold: f32, halo: f32, dark: f32) -> f32 {
    var mask = 1.0;
    if threshold != 0.0 { mask = clamp(abs(d) / threshold, 0.0, 1.0); }
    let k = amount * mask;
    if halo == 0.0 { return clamp(d * k, -0.08, 0.08); }
    let r = abs(d) / halo;
    let shaped = d / (1.0 + r * r * r);
    return k * select(shaped, shaped * dark, shaped < 0.0);
}
@compute @workgroup_size(16, 16)
fn sharpen(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    var blur = 0.0;
    for (var k = 0u; k <= 2u * p.radius; k++) {
        let y = u32(clamp(i32(id.y) + i32(k) - i32(p.radius), 0, i32(p.height) - 1));
        blur += scratch[y * p.width + id.x] * weights[k];
    }
    let i = id.y * p.width + id.x;
    let color = rgb(i);
    let d = luminance(color) - blur;
    let delta = sharpen_delta(d, p.amount, p.threshold, p.halo, p.dark);
    let result = clamp(color + vec3(delta), vec3(0.0), vec3(1.0));
    pixels[3u * i] = result.r;
    pixels[3u * i + 1u] = result.g;
    pixels[3u * i + 2u] = result.b;
}

// `effects::hash`: integer hash to [-1, 1].
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    var v = (bitcast<u32>(x) * 0x9e3779b9u) ^ (bitcast<u32>(y) * 0x85ebca6bu) ^ seed;
    v ^= v >> 16u;
    v *= 0x7feb352du;
    v ^= v >> 15u;
    v *= 0x846ca68bu;
    v ^= v >> 16u;
    return f32(v) / 4294967295.0 * 2.0 - 1.0;
}
fn grain_noise(x: f32, y: f32, size: f32, seed: u32) -> f32 {
    let sx = x / size;
    let sy = y / size;
    let ix = i32(floor(sx));
    let iy = i32(floor(sy));
    let fa = sx - f32(ix);
    let fb = sy - f32(iy);
    let a = fa * fa * (3.0 - 2.0 * fa);
    let b = fb * fb * (3.0 - 2.0 * fb);
    let n = hash(ix, iy, seed) * (1.0 - a) + hash(ix + 1, iy, seed) * a;
    let m = hash(ix, iy + 1, seed) * (1.0 - a) + hash(ix + 1, iy + 1, seed) * a;
    return n * (1.0 - b) + m * b;
}
// `grain::GrainField::strength` of the measured grain: by encoded luminance.
const GRAIN_L = array<f32, 9>(0.0, 0.023, 0.156, 0.41, 0.62, 0.8, 0.91, 0.964, 1.0);
const GRAIN_GAIN = array<f32, 9>(0.0, 0.5, 1.14, 1.08, 1.0, 0.94, 0.86, 0.61, 0.3);
fn grain_strength(l: f32) -> f32 {
    var i = 1u;
    while i < 8u && GRAIN_L[i] <= l {
        i++;
    }
    let t = clamp((l - GRAIN_L[i - 1u]) / (GRAIN_L[i] - GRAIN_L[i - 1u]), 0.0, 1.0);
    return GRAIN_GAIN[i - 1u] + (GRAIN_GAIN[i] - GRAIN_GAIN[i - 1u]) * t;
}
// `film_grain::unit_hash`: integer hash to [0, 1) in 24-bit steps.
fn unit_hash(x: i32, y: i32, seed: u32) -> f32 {
    var v = (bitcast<u32>(x) * 0x9e3779b9u) ^ (bitcast<u32>(y) * 0x85ebca6bu) ^ seed;
    v ^= v >> 16u;
    v *= 0x7feb352du;
    v ^= v >> 15u;
    v *= 0x846ca68bu;
    v ^= v >> 16u;
    return f32(v >> 8u) / 16777216.0;
}
// `film_grain`'s constants.
const FILM_CANDIDATES = 8u;
const FILM_CLUMP_COUNT = 2u;
const FILM_KERNEL = 2.2;
const FILM_STRENGTH = 0.12;
const FILM_COLOR_STRENGTH = 1.1;
const FILM_OUTER_COUPLING = -0.425;
const FILM_TONE_PEAK = 0.428881;
const FILM_WIDTH_PER_RADIUS = 2.0;
const FILM_WIDTH_BASE = 0.8;
const FILM_R2 = vec2(0.7548777, 0.5698403);
const TAU = 6.2831855;
// One `FilmGrainField`: a grain layer.
struct FilmLayer {
    radius: f32, cell: f32, max_radius: f32, norm: f32, seed: u32, sigma: f32, count: u32,
};
// `film_grain::tone_strength`.
fn film_tone_strength(l: f32) -> f32 {
    let t = clamp(1.0 - l, 0.0, 1.0);
    if t <= 0.0 || t >= 1.0 { return 0.0; }
    return t * sqrt(-log(t)) / FILM_TONE_PEAK;
}
// `FilmGrainField::noise`.
fn film_noise(layer: FilmLayer, x: f32, y: f32) -> f32 {
    var sum = 0.0;
    let cell = layer.cell;
    let s = layer.sigma;
    for (var k = 0u; k < layer.count; k++) {
        let ox = fract(0.5 + f32(k) * FILM_R2.x) * cell;
        let oy = fract(0.5 + f32(k) * FILM_R2.y) * cell;
        let lx = x - ox;
        let ly = y - oy;
        let cx = i32(floor(lx / cell));
        let cy = i32(floor(ly / cell));
        let key = layer.seed ^ (k * 0x632be5abu);
        for (var j = -1; j <= 1; j++) {
            for (var i = -1; i <= 1; i++) {
                let gx = cx + i;
                let gy = cy + j;
                let px = (f32(gx) + unit_hash(gx, gy, key ^ 0xb5ad4eceu)) * cell;
                let py = (f32(gy) + unit_hash(gx, gy, key ^ 0x1b873593u)) * cell;
                let d2 = (lx - px) * (lx - px) + (ly - py) * (ly - py);
                if d2 >= cell * cell { continue; }
                let u = 1.0 - unit_hash(gx, gy, key ^ 0xcc9e2d51u);
                let angle = unit_hash(gx, gy, key ^ 0x27d4eb2fu);
                let z = sqrt(-2.0 * log(u)) * cos(TAU * angle);
                let r = min(layer.radius * exp(s * z - 0.5 * s * s), layer.max_radius);
                let reach = FILM_KERNEL * r;
                let b = 1.0 - d2 / (reach * reach);
                if b <= 0.0 { continue; }
                let weight = 2.0 * unit_hash(gx, gy, key ^ 0x68e31da4u) - 1.0;
                sum += weight * b * b;
            }
        }
    }
    return sum * layer.norm;
}
// `FilmGrainField::delta`.
fn film_delta(layer: FilmLayer, x: f32, y: f32, l: f32) -> f32 {
    let strength = film_tone_strength(l);
    if strength == 0.0 { return 0.0; }
    var n = 1u;
    var attenuation = 1.0;
    if p.scale < 1.0 {
        n = select(1u, 2u, p.scale >= 0.5);
        let width = FILM_WIDTH_PER_RADIUS * layer.radius + FILM_WIDTH_BASE;
        attenuation = min(f32(n) * p.scale * width, 1.0);
    }
    let footprint = 1.0 / min(p.scale, 1.0);
    var sum = 0.0;
    for (var j = 0u; j < n; j++) {
        for (var i = 0u; i < n; i++) {
            let ox = ((f32(i) + 0.5) / f32(n) - 0.5) * footprint;
            let oy = ((f32(j) + 0.5) / f32(n) - 0.5) * footprint;
            sum += film_noise(layer, x + ox, y + oy);
        }
    }
    return FILM_STRENGTH * p.film_amount * strength * sum / f32(n * n) * attenuation;
}
// `FilmGrainLayers::delta`: the mono grain over the luminance, and with Color each dye
// layer's grain over its own channel less what they add to the luminance.
fn film_deltas(x: f32, y: f32, c: vec3<f32>) -> vec3<f32> {
    let mono_layer = FilmLayer(p.film_radius, p.film_cell, p.film_max_radius, p.film_norm, p.film_seed, p.film_sigma, FILM_CANDIDATES);
    let mono = film_delta(mono_layer, x, y, luminance(c));
    if p.film_color == 0.0 {
        if mono == 0.0 { return vec3(0.0); }
        return vec3(mono * film_clumps(x, y));
    }
    let red = FilmLayer(
        p.film_red_radius, p.film_red_cell, p.film_red_max_radius, p.film_red_norm, p.film_red_seed,
        p.film_sigma, FILM_CANDIDATES,
    );
    let green = FilmLayer(
        p.film_green_radius, p.film_green_cell, p.film_green_max_radius, p.film_green_norm, p.film_green_seed,
        p.film_sigma, FILM_CANDIDATES,
    );
    let blue = FilmLayer(
        p.film_blue_radius, p.film_blue_cell, p.film_blue_max_radius, p.film_blue_norm, p.film_blue_seed,
        p.film_sigma, FILM_CANDIDATES,
    );
    let r = film_delta(red, x, y, c.r);
    let b = film_delta(blue, x, y, c.b);
    let dye = vec3(r + FILM_OUTER_COUPLING * b, film_delta(green, x, y, c.g), b + FILM_OUTER_COUPLING * r);
    return (vec3(mono) + FILM_COLOR_STRENGTH * p.film_color * (dye - vec3(luminance(dye)))) * film_clumps(x, y);
}
// `FilmGrainLayers::clumps`.
fn film_clumps(x: f32, y: f32) -> f32 {
    let a = p.film_clumping;
    let clump = FilmLayer(
        p.film_clump_radius, p.film_clump_cell, p.film_clump_max_radius, p.film_clump_norm,
        p.film_clump_seed, p.film_clump_sigma, FILM_CLUMP_COUNT,
    );
    return max(1.0 + a * film_noise(clump, x, y), 0.0) / sqrt(1.0 + a * a);
}
// Rust's `round`: halves away from zero (WGSL's `round` goes to even).
fn round_away(v: f32) -> f32 {
    return sign(v) * floor(abs(v) + 0.5);
}
// `pow` for a non-negative base, which WGSL leaves undefined at zero.
fn power(base: f32, exponent: f32) -> f32 {
    if base <= 0.0 { return 0.0; }
    return pow(base, exponent);
}
// `color_math::srgb_decode` and `srgb_encode`.
fn decode(v: f32) -> f32 {
    if v <= 0.04045 { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}
fn encode(v: f32) -> f32 {
    if v <= 0.0031308 { return 12.92 * v; }
    return 1.055 * power(v, 1.0 / 2.4) - 0.055;
}
// `vignette::tone` and `tone_ev`: Camera Raw's neutral tone response and its inverse.
fn tone(ev: f32) -> f32 {
    let last = 24u;
    let at = (ev - TONE_START_EV) / TONE_STEP_EV;
    var log2v = 0.0;
    if at <= 0.0 {
        log2v = TONE_LOG2[0] + ev - TONE_START_EV;
    } else if at < f32(last) {
        let i = u32(at);
        log2v = TONE_LOG2[i] + (TONE_LOG2[i + 1u] - TONE_LOG2[i]) * (at - f32(i));
    }
    return exp2(log2v);
}
fn tone_ev(y: f32) -> f32 {
    let log2v = log2(y);
    let last = 24u;
    if log2v <= TONE_LOG2[0] { return TONE_START_EV + log2v - TONE_LOG2[0]; }
    if log2v >= TONE_LOG2[last] { return TONE_START_EV + f32(last) * TONE_STEP_EV; }
    var lo = 0u;
    var hi = last;
    while hi - lo > 1u {
        let mid = (lo + hi) / 2u;
        if TONE_LOG2[mid] <= log2v { lo = mid; } else { hi = mid; }
    }
    let t = (log2v - TONE_LOG2[lo]) / (TONE_LOG2[hi] - TONE_LOG2[lo]);
    return TONE_START_EV + (f32(lo) + t) * TONE_STEP_EV;
}
// `PostCropVignette::mask`.
fn vignette_mask(nx: f32, ny: f32) -> f32 {
    let q = clamp(
        (power(abs(nx * p.vignette_scale_x), p.vignette_power)
            + power(abs(ny * p.vignette_scale_y), p.vignette_power)) / 2.0,
        1e-6,
        1.0 - 1e-6,
    );
    let z = clamp((log(q / (1.0 - q)) - p.vignette_midpoint) / p.vignette_feather, -40.0, 40.0);
    return 1.0 / (1.0 + exp(-z));
}
fn paint_darken(b: vec3<f32>, amount: f32, mask: f32) -> vec3<f32> {
    return b * (1.0 - (1.0 - decode(1.0 - amount)) * (1.0 - decode(1.0 - mask)));
}
// `PostCropVignette::apply`; styles use Lightroom's codes: 1 Highlight Priority,
// 2 Color Priority, 3 Paint Overlay.
fn vignette(encoded: vec3<f32>, mask: f32) -> vec3<f32> {
    let e = clamp(encoded, vec3(0.0), vec3(1.0));
    let b = vec3(decode(e.r), decode(e.g), decode(e.b));
    let x = abs(p.vignette);
    var out = b;
    if p.vignette_style == 3u {
        if p.vignette < 0.0 {
            out = paint_darken(b, x, mask);
        } else {
            out = b + (vec3(1.0) - b) * (decode(x) * decode(mask));
        }
    } else if p.vignette > 0.0 {
        var strength = 1.1;
        if p.vignette_style == 2u { strength = 0.61; }
        let opacity = strength * decode(x) * decode(mask);
        let white = exp2(SCENE_WHITE_EV);
        for (var c = 0u; c < 3u; c++) {
            let scene = exp2(tone_ev(max(b[c], 1e-12)));
            out[c] = tone(log2(scene + (white - scene) * opacity));
        }
    } else {
        let shape = 1.7625 + 0.6875 * x * x;
        let gain = 1.0 - 0.97 * (1.0 - decode(1.0 - x)) * (1.0 - power(1.0 - mask, shape));
        let y = luminance(b);
        var protect = 1.0;
        if p.vignette_highlights > 0.0 {
            protect = power(1.0 - smoothstep(0.198, 1.108, y), pow(p.vignette_highlights, 1.43));
        }
        let ev = log2(gain) * protect;
        for (var c = 0u; c < 3u; c++) {
            if b[c] > 0.0 { out[c] = tone(tone_ev(b[c]) + ev); } else { out[c] = 0.0; }
        }
        if p.vignette_style == 2u {
            let painted = paint_darken(b, x, mask);
            let w = 0.13 + 0.19 * x;
            for (var c = 0u; c < 3u; c++) {
                out[c] = power(out[c], 1.0 - w) * power(painted[c], w);
            }
        }
    }
    let o = clamp(out, vec3(0.0), vec3(1.0));
    return vec3(encode(o.r), encode(o.g), encode(o.b));
}
fn spatial(color: vec3<f32>, x: u32, y: u32) -> vec3<f32> {
    var c = color;
    let nx = ((f32(x) + 0.5) / f32(p.full_w) - 0.5) * 2.0;
    let ny = ((f32(y) + 0.5) / f32(p.full_h) - 0.5) * 2.0;
    let l = luminance(c);
    if p.vignette != 0.0 {
        c = vignette(c, vignette_mask(nx, ny));
    }
    let lens = clamp(
        max(nx * nx + ny * ny - p.lens_vignette_midpoint, 0.0) / (2.0 - p.lens_vignette_midpoint),
        0.0,
        1.0,
    );
    let gain = exp2(-p.lens_vignette * lens * 2.0);
    var gx = f32(x);
    var gy = f32(y);
    if p.scale != 1.0 {
        gx = (f32(x) + 0.5) / p.scale - 0.5;
        gy = (f32(y) + 0.5) / p.scale - 0.5;
    }
    var noise = 0.0;
    if p.grain != 0.0 {
        var averaged = min(p.grain_cell * p.scale, 1.0);
        var strength = grain_strength(l);
        if p.grain_measured == 0u {
            averaged /= min(p.grain_cell, 1.0);
            strength = 0.13 * max(4.0 * l * (1.0 - l), 0.2);
        }
        let coarse = grain_noise(gx, gy, p.grain_cell, p.grain_seed) * averaged;
        let fine = hash(i32(round_away(gx)), i32(round_away(gy)), p.grain_seed ^ 0x21f09u)
            * min(p.scale, 1.0);
        noise = (coarse * p.grain_coarse + fine * p.grain_fine) * p.grain * strength;
    }
    c = clamp(c * gain + vec3(noise), vec3(0.0), vec3(1.0));
    if p.film_amount != 0.0 {
        c = clamp(c + film_deltas(gx, gy, c), vec3(0.0), vec3(1.0));
    }
    return c;
}
// The monitor profile as a lattice over 8-bit input, `lut_size` points per axis at
// equal byte steps, interpolated trilinearly.
fn lut_at(r: u32, g: u32, b: u32) -> vec3<f32> {
    let i = 3u * ((r * p.lut_size + g) * p.lut_size + b);
    return vec3(lut[i], lut[i + 1u], lut[i + 2u]);
}
fn monitor(bytes: vec3<f32>) -> vec3<f32> {
    let pos = bytes / 255.0 * f32(p.lut_size - 1u);
    let i = min(vec3<u32>(floor(pos)), vec3(p.lut_size - 2u));
    let f = pos - vec3<f32>(i);
    let c00 = mix(lut_at(i.x, i.y, i.z), lut_at(i.x + 1u, i.y, i.z), f.x);
    let c10 = mix(lut_at(i.x, i.y + 1u, i.z), lut_at(i.x + 1u, i.y + 1u, i.z), f.x);
    let c01 = mix(lut_at(i.x, i.y, i.z + 1u), lut_at(i.x + 1u, i.y, i.z + 1u), f.x);
    let c11 = mix(lut_at(i.x, i.y + 1u, i.z + 1u), lut_at(i.x + 1u, i.y + 1u, i.z + 1u), f.x);
    return floor(mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z) + 0.5);
}
@compute @workgroup_size(16, 16)
fn present(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.crop_w || id.y >= p.crop_h { return; }
    let bx = p.crop_x + id.x;
    let by = p.crop_y + id.y;
    var color = rgb(by * p.width + bx);
    if p.effects != 0u {
        color = spatial(color, p.origin_x + bx, p.origin_y + by);
    }
    let clamped = clamp(color, vec3(0.0), vec3(1.0));
    if p.count != 0u {
        let bins = vec3<u32>(clamped * 255.0);
        atomicAdd(&histogram[bins.r], 1u);
        atomicAdd(&histogram[256u + bins.g], 1u);
        atomicAdd(&histogram[512u + bins.b], 1u);
        // `Clipped`: per channel, highlights then shadows, at `HIGHLIGHT_CLIP`
        // and `SHADOW_CLIP` of the rendered values, before the monitor profile.
        for (var c = 0u; c < 3u; c++) {
            if color[c] >= 0.999 { atomicAdd(&histogram[768u + c], 1u); }
            if color[c] <= 0.001 { atomicAdd(&histogram[771u + c], 1u); }
        }
    }
    var bytes = floor(clamped * 255.0 + 0.5);
    if p.lut_size > 1u {
        bytes = monitor(bytes);
    }
    // `ClipOverlay`: 1 highlights, 2 shadows.
    if (p.clipping & 1u) != 0u && any(color >= vec3(0.999)) {
        bytes = vec3(255.0, 40.0, 40.0);
    } else if (p.clipping & 2u) != 0u && all(color <= vec3(0.001)) {
        bytes = vec3(40.0, 80.0, 255.0);
    }
    textureStore(shown, vec2(id.x, id.y), vec4(bytes / 255.0, 1.0));
}
