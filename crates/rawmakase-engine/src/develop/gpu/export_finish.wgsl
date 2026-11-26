// Export finishing after the develop stage, per horizontal band: OutputSpace-aware
// sharpening, Post-Crop Vignette (all three styles), Film Grain, separable Lanczos3
// resizing, and direct rgb8 / rgb16 packing. Ports of `quality::sharpen_with_radius`,
// `effects::spatial_finish_scaled`, `quality::resize`, and `Rendered::rgb8`/`rgb16`;
// the CPU versions are the reference.
struct Params {
    // Full-res band buffer dimensions and valid vertical window.
    width: u32, height: u32, crop_y: u32, crop_h: u32,
    // Band output dimensions (equal to width, crop_h when unresized).
    out_width: u32, out_height: u32,
    // First global output row of this band (for Lanczos Y lookup).
    out_y0: u32,
    // First global full-res row held in `pixels` (`y_comp0`).
    origin_y: u32,
    // Whole full-res output size (for vignette and grain coordinates).
    full_w: u32, full_h: u32,
    // OutputSpace code: 0 = Srgb, 1 = AdobeRgb, 2 = ProPhoto.
    output_space: u32,
    // 1 when Lanczos3 resizing runs (final f32 in `output`), 0 when unresized (in `pixels`).
    resize: u32,
    // Sharpening parameters (`sharpening::Sharpener`).
    radius: u32, amount: f32, threshold: f32, halo: f32,
    dark: f32,
    // Lanczos weight table offsets and strides inside `weights`.
    x_offset: u32, x_stride: u32, y_offset: u32,
    y_stride: u32,
    // Direct packing parameters.
    pack_src_offset: u32, pack_groups_x: u32,
    // Spatial effects (`effects::GrainField` and `effects::PostCropVignette`).
    scale: f32,
    grain: f32, grain_cell: f32, grain_coarse: f32, grain_fine: f32,
    grain_seed: u32, vignette: f32, vignette_style: u32, vignette_highlights: f32,
    vignette_scale_x: f32, vignette_scale_y: f32, vignette_power: f32, vignette_midpoint: f32,
    vignette_feather: f32, hdr_peak: f32, hdr_decode: u32, pad2: u32,
};

@group(0) @binding(0) var<storage, read_write> pixels: array<f32>;
@group(0) @binding(1) var<storage, read_write> scratch: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
@group(0) @binding(3) var<storage, read> weights: array<f32>;
@group(0) @binding(4) var<uniform> p: Params;
@group(0) @binding(5) var<storage, read_write> packed_out: array<u32>;

const ADOBE_GAMMA: f32 = 563.0 / 256.0;
const ROMM_LINEAR: f32 = 1.0 / 512.0;
const ROMM_ENCODED: f32 = 1.0 / 32.0;

fn rgb(i: u32) -> vec3<f32> {
    return vec3(pixels[3u * i], pixels[3u * i + 1u], pixels[3u * i + 2u]);
}

// `OutputSpace::luminance`: Rec. 709 for sRGB, Y rows of Adobe RGB / ProPhoto RGB-to-XYZ.
fn luminance(v: vec3<f32>) -> f32 {
    if p.output_space == 1u {
        return 0.29734498 * v.r + 0.62736357 * v.g + 0.07529146 * v.b;
    }
    if p.output_space == 2u {
        return 0.28807113 * v.r + 0.7118432 * v.g + 0.000085654 * v.b;
    }
    return 0.2126 * v.r + 0.7152 * v.g + 0.0722 * v.b;
}

// `pow` for a non-negative base, which WGSL leaves undefined at zero.
fn power(base: f32, exponent: f32) -> f32 {
    if base <= 0.0 { return 0.0; }
    return pow(base, exponent);
}

// `rawmakase_model::color::srgb_decode` and `srgb_encode`.
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}
fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 { return 12.92 * v; }
    return 1.055 * power(v, 1.0 / 2.4) - 0.055;
}

// `OutputSpace::decode` and `OutputSpace::encode`.
fn decode_space(v: f32) -> f32 {
    if p.output_space == 1u {
        return power(v, ADOBE_GAMMA);
    }
    if p.output_space == 2u {
        if v < ROMM_ENCODED { return v / 16.0; }
        return power(v, 1.8);
    }
    return srgb_decode(v);
}
fn encode_space(v: f32) -> f32 {
    if p.output_space == 1u {
        return power(v, 1.0 / ADOBE_GAMMA);
    }
    if p.output_space == 2u {
        if v < ROMM_LINEAR { return 16.0 * v; }
        return power(v, 1.0 / 1.8);
    }
    return srgb_encode(v);
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

// `grain::hash`: integer hash to [-1, 1].
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

// Sliders and mask use sRGB decoding in every OutputSpace, matching `vignette.rs`.
fn paint_darken(b: vec3<f32>, amount: f32, mask: f32) -> vec3<f32> {
    return b * (1.0 - (1.0 - srgb_decode(1.0 - amount)) * (1.0 - srgb_decode(1.0 - mask)));
}

// `PostCropVignette::apply`; styles use Lightroom's codes: 1 Highlight Priority,
// 2 Color Priority, 3 Paint Overlay.
fn vignette(encoded: vec3<f32>, mask: f32) -> vec3<f32> {
    let e = clamp(encoded, vec3(0.0), vec3(1.0));
    let b = vec3(decode_space(e.r), decode_space(e.g), decode_space(e.b));
    let x = abs(p.vignette);
    var out = b;
    if p.vignette_style == 3u {
        if p.vignette < 0.0 {
            out = paint_darken(b, x, mask);
        } else {
            out = b + (vec3(1.0) - b) * (srgb_decode(x) * srgb_decode(mask));
        }
    } else if p.vignette > 0.0 {
        var strength = 1.1;
        if p.vignette_style == 2u { strength = 0.61; }
        let opacity = strength * srgb_decode(x) * srgb_decode(mask);
        let white = exp2(SCENE_WHITE_EV);
        for (var c = 0u; c < 3u; c++) {
            let scene = exp2(tone_ev(max(b[c], 1e-12)));
            out[c] = tone(log2(scene + (white - scene) * opacity));
        }
    } else {
        let shape = 1.7625 + 0.6875 * x * x;
        let gain = 1.0 - 0.97 * (1.0 - srgb_decode(1.0 - x)) * (1.0 - power(1.0 - mask, shape));
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
    return vec3(encode_space(o.r), encode_space(o.g), encode_space(o.b));
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
        let averaged = min(p.grain_cell * p.scale, 1.0);
        let strength = grain_strength(l);
        let coarse = grain_noise(gx, gy, p.grain_cell, p.grain_seed) * averaged;
        let fine = hash(i32(round_away(gx)), i32(round_away(gy)), p.grain_seed ^ 0x21f09u)
            * min(p.scale, 1.0);
        noise = (coarse * p.grain_coarse + fine * p.grain_fine) * p.grain * strength;
    }
    return clamp(c + vec3(noise), vec3(0.0), vec3(1.0));
}

// Applies Post-Crop Vignette and Film Grain in place on the valid full-res rows
// `[p.crop_y .. p.crop_y + p.crop_h)` of this band.
@compute @workgroup_size(16, 16)
fn spatial_effects(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.crop_h { return; }
    let by = p.crop_y + id.y;
    let i = by * p.width + id.x;
    let result = spatial(rgb(i), id.x, p.origin_y + by);
    pixels[3u * i] = result.r;
    pixels[3u * i + 1u] = result.g;
    pixels[3u * i + 2u] = result.b;
}

@compute @workgroup_size(16, 16)
fn resize_vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.out_height { return; }
    let oy = p.out_y0 + id.y;
    let offset = p.y_offset + oy * p.y_stride;
    let start = u32(weights[offset]);
    let count = u32(weights[offset + 1u]);
    let local_start = start - p.origin_y;
    var sum = vec3(0.0);
    for (var k = 0u; k < count; k++) {
        sum += rgb((local_start + k) * p.width + id.x) * weights[offset + 2u + k];
    }
    let i = 3u * (id.y * p.width + id.x);
    scratch[i] = sum.r;
    scratch[i + 1u] = sum.g;
    scratch[i + 2u] = sum.b;
}

@compute @workgroup_size(16, 16)
fn resize_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.out_width || id.y >= p.out_height { return; }
    let offset = p.x_offset + id.x * p.x_stride;
    let start = u32(weights[offset]);
    let count = u32(weights[offset + 1u]);
    var sum = vec3(0.0);
    for (var k = 0u; k < count; k++) {
        let i = 3u * (id.y * p.width + start + k);
        sum += vec3(scratch[i], scratch[i + 1u], scratch[i + 2u]) * weights[offset + 2u + k];
    }
    let result = clamp(sum, vec3(0.0), vec3(1.0));
    let i = 3u * (id.y * p.out_width + id.x);
    output[i] = result.r;
    output[i + 1u] = result.g;
    output[i + 2u] = result.b;
}

fn pack_sample(idx: u32) -> f32 {
    if p.resize != 0u {
        return output[idx];
    }
    return pixels[p.pack_src_offset + idx];
}

// Packs `out_width * out_height * 3` float samples into tightly packed little-endian
// 8-bit RGB words, using `floor(clamp(v, 0.0, 1.0) * 255.0 + 0.5)` matching Rust's
// `unit_to_u8`.
@compute @workgroup_size(256)
fn pack_rgb8(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let word = (group.y * p.pack_groups_x + group.x) * 256u + local;
    let total = p.out_width * p.out_height * 3u;
    let base = word * 4u;
    if base >= total { return; }
    var packed = 0u;
    for (var j = 0u; j < 4u; j++) {
        let idx = base + j;
        if idx < total {
            let v = clamp(pack_sample(idx), 0.0, 1.0);
            let byte = u32(floor(v * 255.0 + 0.5));
            packed |= byte << (8u * j);
        }
    }
    packed_out[word] = packed;
}

// Packs `out_width * out_height * 3` float samples into tightly packed little-endian
// 16-bit RGB words, using `floor(clamp(v, 0.0, 1.0) * 65535.0 + 0.5)` matching Rust's
// `unit_to_u16`.
@compute @workgroup_size(256)
fn pack_rgb16(
    @builtin(workgroup_id) group: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let word = (group.y * p.pack_groups_x + group.x) * 256u + local;
    let total = p.out_width * p.out_height * 3u;
    let base = word * 2u;
    if base >= total { return; }
    let v0 = clamp(pack_sample(base), 0.0, 1.0);
    var packed = u32(floor(v0 * 65535.0 + 0.5));
    if base + 1u < total {
        let v1 = clamp(pack_sample(base + 1u), 0.0, 1.0);
        packed |= u32(floor(v1 * 65535.0 + 0.5)) << 16u;
    }
    packed_out[word] = packed;
}

// ============================================================================
// HDR master finishing (`quality::hdr::master`: unclipped sRGB-curve sharpen,
// unclipped Lanczos3 resize, sRGB-curve decode, and `roll_off` to `hdr_peak`)
// ============================================================================

// `color::hdr_output::roll_off`: at most `peak`, unchanged below 97% of it.
fn roll_off(v: f32, peak: f32) -> f32 {
    let knee = 0.97 * peak;
    if !(v == v) {
        return 0.0;
    }
    if v <= knee {
        return v;
    }
    let room = peak - knee;
    return peak - room * exp(-(v - knee) / room);
}

// Encodes linear ProPhoto HDR master pixels onto the extended sRGB curve in place
// before unbounded sharpening and/or Lanczos3 resizing (`quality::hdr::master`).
@compute @workgroup_size(16, 16)
fn hdr_srgb_encode(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    let i = 3u * (id.y * p.width + id.x);
    pixels[i] = srgb_encode(max(pixels[i], 0.0));
    pixels[i + 1u] = srgb_encode(max(pixels[i + 1u], 0.0));
    pixels[i + 2u] = srgb_encode(max(pixels[i + 2u], 0.0));
}

// `quality::sharpen_unbounded`: adds luminance detail unclipped above white (clamped at 0).
@compute @workgroup_size(16, 16)
fn hdr_sharpen(@builtin(global_invocation_id) id: vec3<u32>) {
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
    let result = max(color + vec3(delta), vec3(0.0));
    pixels[3u * i] = result.r;
    pixels[3u * i + 1u] = result.g;
    pixels[3u * i + 2u] = result.b;
}

// `quality::resize_unbounded` horizontal pass: clamps only negative Lanczos ringing at 0,
// keeping highlights unclipped above white.
@compute @workgroup_size(16, 16)
fn hdr_resize_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.out_width || id.y >= p.out_height { return; }
    let offset = p.x_offset + id.x * p.x_stride;
    let start = u32(weights[offset]);
    let count = u32(weights[offset + 1u]);
    var sum = vec3(0.0);
    for (var k = 0u; k < count; k++) {
        let i = 3u * (id.y * p.width + start + k);
        sum += vec3(scratch[i], scratch[i + 1u], scratch[i + 2u]) * weights[offset + 2u + k];
    }
    let result = max(sum, vec3(0.0));
    let i = 3u * (id.y * p.out_width + id.x);
    output[i] = result.r;
    output[i + 1u] = result.g;
    output[i + 2u] = result.b;
}

// Decodes sRGB-curve values back to linear ProPhoto (when `hdr_decode != 0`) and applies
// `roll_off(v, hdr_peak)` in place on the band's output buffer (`output` when resized,
// `pixels` at `crop_y` otherwise).
@compute @workgroup_size(16, 16)
fn hdr_finish_master(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.out_width || id.y >= p.out_height { return; }
    if p.resize != 0u {
        let i = 3u * (id.y * p.out_width + id.x);
        var c = vec3(output[i], output[i + 1u], output[i + 2u]);
        if p.hdr_decode != 0u {
            c = vec3(srgb_decode(max(c.r, 0.0)), srgb_decode(max(c.g, 0.0)), srgb_decode(max(c.b, 0.0)));
        }
        output[i] = roll_off(c.r, p.hdr_peak);
        output[i + 1u] = roll_off(c.g, p.hdr_peak);
        output[i + 2u] = roll_off(c.b, p.hdr_peak);
    } else {
        let i = 3u * ((p.crop_y + id.y) * p.width + id.x);
        var c = vec3(pixels[i], pixels[i + 1u], pixels[i + 2u]);
        if p.hdr_decode != 0u {
            c = vec3(srgb_decode(max(c.r, 0.0)), srgb_decode(max(c.g, 0.0)), srgb_decode(max(c.b, 0.0)));
        }
        pixels[i] = roll_off(c.r, p.hdr_peak);
        pixels[i + 1u] = roll_off(c.g, p.hdr_peak);
        pixels[i + 2u] = roll_off(c.b, p.hdr_peak);
    }
}
