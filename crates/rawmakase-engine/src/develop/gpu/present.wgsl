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
    vignette_feather: f32, effects: u32, count: u32, halo: f32,
    dark: f32, grain_fine: f32, pad3: u32, pad4: u32,
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
// applied): the high-pass shaped, dark halos at `dark` of light ones.
fn sharpen_delta(d: f32, amount: f32, threshold: f32, halo: f32, dark: f32) -> f32 {
    var mask = 1.0;
    if threshold != 0.0 { mask = clamp(abs(d) / threshold, 0.0, 1.0); }
    let k = amount * mask;
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
// `grain::GrainField::strength`: by encoded luminance.
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
    var gx = f32(x);
    var gy = f32(y);
    if p.scale != 1.0 {
        gx = (f32(x) + 0.5) / p.scale - 0.5;
        gy = (f32(y) + 0.5) / p.scale - 0.5;
    }
    var noise = 0.0;
    if p.grain != 0.0 {
        // Grain below a pixel wide averages away within the pixel.
        let averaged = min(p.grain_cell * p.scale, 1.0);
        let strength = grain_strength(l);
        let coarse = grain_noise(gx, gy, p.grain_cell, p.grain_seed) * averaged;
        let fine = hash(i32(round_away(gx)), i32(round_away(gy)), p.grain_seed ^ 0x21f09u)
            * min(p.scale, 1.0);
        noise = (coarse * p.grain_coarse + fine * p.grain_fine) * p.grain * strength;
    }
    return clamp(c + vec3(noise), vec3(0.0), vec3(1.0));
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
