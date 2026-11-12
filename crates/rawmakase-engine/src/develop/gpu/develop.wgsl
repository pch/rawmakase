// The per-pixel color and tone stage: a port of `pipeline::process_pixel` for recipes
// with a camera profile (see `pixel_params.rs`).
// The CPU implementation is the reference; functions keep its names and order.
// `P_*` indices into `params` are generated from `pixel_params::slot`.

@group(0) @binding(0) var<storage, read> samples: array<f32>;
@group(0) @binding(1) var<storage, read> positions: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
@group(0) @binding(3) var<storage, read> params: array<f32>;
@group(0) @binding(4) var<storage, read> tables: array<f32>;
@group(0) @binding(5) var<uniform> size: vec4<u32>;
// Mask weights: one byte per mask, four per word, `P_MASK_WORDS` words per pixel.
@group(0) @binding(6) var<storage, read> weights: array<u32>;
// The Shadows/Highlights map's grids (`local_tone::MapGrid`), kept on the device per map.
@group(0) @binding(7) var<storage, read> grid: array<f32>;
// The camera colour per sample, kept between renders that change only the stages after
// it: `size.z` 1 reads it, 2 computes and writes it, 0 neither (see `Kept`).
@group(0) @binding(8) var<storage, read_write> kept: array<f32>;

const TO_2020 = mat3x3<f32>(
    vec3(0.627404, 0.069097, 0.016391),
    vec3(0.329283, 0.91954, 0.088013),
    vec3(0.043313, 0.011362, 0.895595),
);
const FROM_2020 = mat3x3<f32>(
    vec3(1.660491, -0.12455, -0.018151),
    vec3(-0.587641, 1.1329, -0.100579),
    vec3(-0.07285, -0.008349, 1.11873),
);
const PRO_TO_RGB = mat3x3<f32>(
    vec3(2.034075, -0.228813, -0.00857),
    vec3(-0.727334, 1.231731, -0.153286),
    vec3(-0.306742, -0.002918, 1.161856),
);
const RGB_TO_PRO = mat3x3<f32>(
    vec3(0.529345, 0.098374, 0.016883),
    vec3(0.330072, 0.873462, 0.117673),
    vec3(0.140583, 0.028164, 0.865444),
);
const TAU: f32 = 6.28318530717958647692;
const OUTSIDE: f32 = -1e38;

fn p(i: u32) -> f32 {
    return params[i];
}

// masks::local: the pixel's summed mask adjustments, and whether any mask reaches it.
const LOCAL_LEN: u32 = 18u;
const L_TEMPERATURE: u32 = 0u;
const L_TINT: u32 = 1u;
const L_EXPOSURE: u32 = 2u;
const L_CONTRAST: u32 = 3u;
const L_HIGHLIGHTS: u32 = 4u;
const L_SHADOWS: u32 = 5u;
const L_WHITES: u32 = 6u;
const L_BLACKS: u32 = 7u;
const L_DEHAZE: u32 = 10u;
const L_HUE: u32 = 11u;
const L_SATURATION: u32 = 12u;
const L_COLOR: u32 = 15u;
var<private> delta: array<f32, 18>;
var<private> masked: bool;
fn load_delta(i: u32) {
    masked = false;
    if !V_MASKS {
        return;
    }
    for (var k = 0u; k < LOCAL_LEN; k++) {
        delta[k] = 0.0;
    }
    let n = u32(p(P_MASKS));
    if n == 0u {
        return;
    }
    let words = u32(p(P_MASK_WORDS));
    let base = offset(P_MASK_DELTAS);
    for (var g = 0u; g < n; g++) {
        let byte = (weights[i * words + g / 4u] >> (8u * (g % 4u))) & 255u;
        if byte > 0u {
            masked = true;
            let w = f32(byte) / 255.0;
            for (var k = 0u; k < LOCAL_LEN; k++) {
                delta[k] += table(base + i32(g * LOCAL_LEN + k)) * w;
            }
        }
    }
}
fn offset(i: u32) -> i32 {
    return i32(params[i]);
}
/// Row-major 3×3 matrix stored at `params[i..i + 9]`.
fn matrix(i: u32) -> mat3x3<f32> {
    return transpose(mat3x3<f32>(
        vec3(p(i), p(i + 1u), p(i + 2u)),
        vec3(p(i + 3u), p(i + 4u), p(i + 5u)),
        vec3(p(i + 6u), p(i + 7u), p(i + 8u)),
    ));
}
fn rem_euclid(x: f32, y: f32) -> f32 {
    let r = x - trunc(x / y) * y;
    return select(r, r + abs(y), r < 0.0);
}
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return select(0.0, 1.0, y == 0.0);
    }
    return pow(x, y);
}
fn cbrt(x: f32) -> f32 {
    return sign(x) * powf(abs(x), 1.0 / 3.0);
}
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return powf((v + 0.055) / 1.055, 2.4);
}
fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        return 12.92 * v;
    }
    return 1.055 * powf(max(v, 0.0), 1.0 / 2.4) - 0.055;
}
fn table(i: i32) -> f32 {
    return tables[u32(i)];
}
fn table3(i: i32) -> vec3<f32> {
    return vec3(table(i), table(i + 1), table(i + 2));
}
fn hsv_to_rgb(h: f32, v: f32, s: f32) -> vec3<f32> {
    let c = v * s;
    let x = c * (1.0 - abs(h % 2.0 - 1.0));
    var rgb: vec3<f32>;
    switch u32(h) {
        case 0u: { rgb = vec3(c, x, 0.0); }
        case 1u: { rgb = vec3(x, c, 0.0); }
        case 2u: { rgb = vec3(0.0, c, x); }
        case 3u: { rgb = vec3(0.0, x, c); }
        case 4u: { rgb = vec3(x, 0.0, c); }
        default: { rgb = vec3(c, 0.0, x); }
    }
    return rgb;
}
fn hue_of(rgb: vec3<f32>, max_v: f32, d: f32) -> f32 {
    if d < 1e-9 {
        return 0.0;
    } else if max_v == rgb.x {
        return rem_euclid((rgb.y - rgb.z) / d, 6.0) / 6.0;
    } else if max_v == rgb.y {
        return ((rgb.z - rgb.x) / d + 2.0) / 6.0;
    }
    return ((rgb.x - rgb.y) / d + 4.0) / 6.0;
}

// camera_profiles::Table
struct Table {
    data: i32,
    dims: vec3<u32>,
    srgb: bool,
}
fn table_at(i: u32) -> Table {
    return Table(offset(i), vec3(u32(p(i + 1u)), u32(p(i + 2u)), u32(p(i + 3u))), p(i + 4u) != 0.0);
}
fn table_lookup(t: Table, h: f32, s: f32, v: f32) -> vec3<f32> {
    let coords = vec3(
        rem_euclid(h, 1.0) * f32(t.dims.x),
        clamp(s, 0.0, 1.0) * f32(t.dims.y - 1u),
        clamp(v, 0.0, 1.0) * f32(t.dims.z - 1u),
    );
    let base = vec3<u32>(floor(coords));
    let f = coords - vec3<f32>(base);
    var out = vec3(0.0);
    for (var z = 0u; z < 2u; z++) {
        for (var y = 0u; y < 2u; y++) {
            for (var x = 0u; x < 2u; x++) {
                let ix = (base.x + x) % t.dims.x;
                let iy = min(base.y + y, t.dims.y - 1u);
                let iz = min(base.z + z, t.dims.z - 1u);
                let wx = select(1.0 - f.x, f.x, x == 1u);
                let wy = select(1.0 - f.y, f.y, y == 1u);
                let wz = select(1.0 - f.z, f.z, z == 1u);
                let cell = (iz * t.dims.x + ix) * t.dims.y + iy;
                out += table3(t.data + i32(cell) * 3) * (wx * wy * wz);
            }
        }
    }
    return out;
}
fn table_apply(t: Table, rgb: vec3<f32>, other: i32, weight: f32) -> vec3<f32> {
    let max_v = max(max(max(rgb.x, rgb.y), rgb.z), 0.0);
    let min_v = max(min(min(rgb.x, rgb.y), rgb.z), 0.0);
    let d = max_v - min_v;
    let h = hue_of(rgb, max_v, d);
    let sat = select(0.0, d / max_v, max_v > 0.0);
    let encoded = t.srgb && t.dims.z > 1u;
    let v = select(max_v, srgb_encode(max_v), encoded);
    var delta = table_lookup(t, h, sat, v);
    if other >= 0 {
        let b = table_lookup(Table(other, t.dims, t.srgb), h, sat, v);
        delta = delta * (1.0 - weight) + b * weight;
    }
    let h2 = rem_euclid(h + delta.x / 360.0, 1.0) * 6.0;
    let s = clamp(sat * delta.y, 0.0, 1.0);
    let v2 = select(v * delta.z, srgb_decode(v * delta.z), encoded);
    let c = v2 * s;
    return hsv_to_rgb(h2, v2, s) + (v2 - c);
}
/// Piecewise-linear profile tone curve: `partition_point` then interpolation.
fn tone_eval(v: f32) -> f32 {
    let base = offset(P_TONE);
    let n = u32(p(P_TONE_COUNT));
    let x = clamp(v, 0.0, 1.0);
    var lo = 0u;
    var hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if table(base + i32(mid) * 2) <= x {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let i = min(max(lo, 1u) - 1u, n - 2u);
    let a = vec2(table(base + i32(i) * 2), table(base + i32(i) * 2 + 1));
    let b = vec2(table(base + i32(i) * 2 + 2), table(base + i32(i) * 2 + 3));
    let t = clamp((x - a.x) / (b.x - a.x), 0.0, 1.0);
    return a.y * (1.0 - t) + b.y * t;
}
/// A 0–1 lookup table of `size + 1` samples, linearly interpolated.
fn lut(base: i32, size: u32, v: f32) -> f32 {
    let f = clamp(v, 0.0, 1.0) * f32(size);
    let i = min(u32(f), size - 1u);
    let a = table(base + i32(i));
    return a + (table(base + i32(i) + 1) - a) * (f - f32(i));
}
/// DNG RGBTone: map the brightest and darkest channels, keep the middle one's position.
fn rgb_tone_values(q: vec3<f32>, a: f32, b: f32, lo: f32, hi: f32) -> vec3<f32> {
    if hi - lo > 1e-8 {
        return a + (b - a) * (q - lo) / (hi - lo);
    }
    return vec3(a);
}
fn enhanced_curve(rgb: vec3<f32>) -> vec3<f32> {
    let q = vec3(
        srgb_encode(clamp(rgb.x, 0.0, 1.0)),
        srgb_encode(clamp(rgb.y, 0.0, 1.0)),
        srgb_encode(clamp(rgb.z, 0.0, 1.0)),
    );
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(max(q.x, q.y), q.z), 0.0);
    let base = offset(P_ENH_CURVE);
    let r = rgb_tone_values(q, lut(base, 4096u, lo), lut(base, 4096u, hi), lo, hi);
    return vec3(srgb_decode(r.x), srgb_decode(r.y), srgb_decode(r.z));
}
// camera_profiles::overrange: DNG 1.7's encoding of linear values, white at about
// 0.502 and 16 at 1, which an HDR profile's tables and curve read.
fn overrange_encode(x_in: f32) -> f32 {
    let x = max(x_in, 0.0);
    return x * (256.0 + x) / (256.0 * (1.0 + x));
}
fn overrange_decode(y_in: f32) -> f32 {
    let y = max(y_in, 0.0);
    return 16.0 * y / (sqrt(64.0 * y * y - 127.0 * y + 64.0) + 8.0 - 8.0 * y);
}
// Table::apply_overrange: a 3D table looks up overrange-encoded values; a 2D one
// (a single value division) the values as they are, not negative.
fn table_apply_overrange(t: Table, rgb: vec3<f32>, other: i32, weight: f32) -> vec3<f32> {
    if t.dims.z > 1u {
        let e = vec3(overrange_encode(rgb.x), overrange_encode(rgb.y), overrange_encode(rgb.z));
        let out = table_apply(t, e, other, weight);
        return vec3(overrange_decode(out.x), overrange_decode(out.y), overrange_decode(out.z));
    }
    return table_apply(t, max(rgb, vec3(0.0)), other, weight);
}
// An HDR profile's curve on an overrange-encoded value: the profile's curve up to 1,
// and on with its slope at 1 above it (`CameraProfile::finish_hdr`).
fn overrange_tone(x: f32) -> f32 {
    if x <= 1.0 {
        return tone_eval(x);
    }
    let top = tone_eval(1.0);
    return top + (x - 1.0) * (top - tone_eval(0.999)) / 0.001;
}
// hdr_look::encode and decode: the sRGB curve up to white, a log curve above it.
fn look_encode(x: f32) -> f32 {
    if x <= 1.0 {
        return srgb_encode(max(x, 0.0));
    }
    return 1.0 + LOOK_SLOPE / LOOK_BEND * log2(1.0 + LOOK_BEND * log2(x));
}
fn look_decode(e: f32) -> f32 {
    if e <= 1.0 {
        return srgb_decode(max(e, 0.0));
    }
    return exp2((exp2((e - 1.0) * LOOK_BEND / LOOK_SLOPE) - 1.0) / LOOK_BEND);
}
// LookTone::evaluate: the look's curve (`evaluate_as_look`) on an encoded value, and
// above where it ends, its lift held or its last slope.
fn look_eval(e: f32) -> f32 {
    let curve = offset(P_ENH_CURVE);
    let kind = p(P_HDR_LOOK);
    if kind == 1.0 {
        if e <= p(P_HDR_LOOK + 1u) {
            return lut(curve, 4096u, e);
        }
        return min(e + p(P_HDR_LOOK + 2u), look_encode(LOOK_PEAK));
    }
    if e <= 1.0 {
        return lut(curve, 4096u, e);
    }
    return lut(curve, 4096u, 1.0) + p(P_HDR_LOOK + 1u) * (e - 1.0);
}
// LookTone::apply: DNG RGBTone with the look's curve on encoded values.
fn look_apply(rgb: vec3<f32>) -> vec3<f32> {
    let q = vec3(look_encode(rgb.x), look_encode(rgb.y), look_encode(rgb.z));
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(max(q.x, q.y), q.z), 0.0);
    let a = look_eval(lo);
    let b = look_eval(hi);
    var out = vec3(b);
    if hi - lo > 1e-8 {
        out = a + (b - a) * (q - lo) / (hi - lo);
    }
    return vec3(look_decode(out.x), look_decode(out.y), look_decode(out.z));
}
// RGBTone on look-encoded values (`hdr_look::encode`), the darkest and brightest
// channels through `curve` (0 Contrast, 1 the parametric curve, 2 the master curve).
fn encoded_tone(q: vec3<f32>, curve: u32) -> vec3<f32> {
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(max(q.x, q.y), q.z), 0.0);
    let a = encoded_curve(curve, lo);
    let b = encoded_curve(curve, hi);
    if hi - lo > 1e-8 {
        return a + (b - a) * (q - lo) / (hi - lo);
    }
    return vec3(b);
}
fn encoded_curve(curve: u32, e: f32) -> f32 {
    if curve == 0u {
        // HdrContrast::evaluate: its table up to four stops above white, then on along
        // its slope in linear light.
        let end = look_encode(HDR_CONTRAST_TOP);
        if e > end {
            return look_encode(p(P_HDR_CONTRAST + 1u) + p(P_HDR_CONTRAST + 2u) * (look_decode(e) - HDR_CONTRAST_TOP));
        }
        let f = max(e, 0.0) / end * HDR_CONTRAST_SAMPLES;
        let i = min(floor(f), HDR_CONTRAST_SAMPLES - 1.0);
        let base = offset(P_HDR_CONTRAST) + i32(i);
        return mix(table(base), table(base + 1), f - i);
    }
    if curve == 1u {
        // hdr_curve::parametric: the curve over 0–encode(16), the identity above.
        let top = look_encode(LOOK_PEAK);
        if e >= top {
            return e;
        }
        return lut(offset(P_PARAMETRIC_LUT), 1024u, max(e, 0.0) / top) * top;
    }
    // HdrPointCurve::evaluate: the curve up to white, then on along its end slope,
    // held above four stops over white (hdr_curve::hold) unless it is the identity.
    var out = 0.0;
    if e <= 1.0 {
        out = lut(offset(P_MASTER), 4096u, e);
    } else {
        out = p(P_HDR_CURVES + 2u) + p(P_HDR_CURVES + 3u) * (e - 1.0);
    }
    if p(P_HDR_CURVES + 4u) == 0.0 {
        return out;
    }
    let knee = look_encode(LOOK_PEAK);
    if out <= knee {
        return out;
    }
    let room = HDR_CURVE_HOLD - knee;
    return knee + room * (1.0 - exp(-(out - knee) / room));
}
// HdrCurves::apply's last step on channel `c`'s encoded value: its own curve, when it
// applies (hdr_curve::channel: the samples up to white, then on along its end slope,
// held at 16), else the master curve's output decoded.
fn hdr_channel(c: u32, e: f32) -> f32 {
    let at = P_HDR_CHANNELS + 3u * c;
    if p(at) == 0.0 {
        return min(look_decode(e), HDR_CURVE_CEILING);
    }
    var q = 0.0;
    if e <= 1.0 {
        q = lut(offset(P_CHANNELS + c), 4096u, e);
    } else {
        q = p(at + 1u) + p(at + 2u) * (e - 1.0);
    }
    return min(look_decode(q), LOOK_PEAK);
}
// hdr_curve::levels on a look-encoded value: the SDR Levels up to white, then on
// along their slope there.
fn hdr_levels(e: f32) -> f32 {
    let black = p(P_LEVELS);
    let white = p(P_LEVELS + 1u);
    let midtone = p(P_LEVELS + 2u);
    let at = min(e, 1.0);
    let sdr = pow(clamp((at - black) / (white - black), 0.0, 1.0), 1.0 / midtone);
    if e <= 1.0 {
        return sdr;
    }
    var slope = 0.0;
    if white >= 1.0 {
        slope = 1.0 / (midtone * (1.0 - black));
    }
    return sdr + slope * (e - 1.0);
}
// HdrContrast::apply, then HdrCurves::apply: linear ProPhoto, not negative, in and out.
fn hdr_tone_controls(rgb: vec3<f32>) -> vec3<f32> {
    var out = rgb;
    if offset(P_HDR_CONTRAST) >= 0 {
        let q = vec3(look_encode(out.x), look_encode(out.y), look_encode(out.z));
        let c = encoded_tone(q, 0u);
        out = max(vec3(look_decode(c.x), look_decode(c.y), look_decode(c.z)), vec3(0.0));
    }
    if p(P_HDR_CURVES) != 0.0 {
        var q = vec3(look_encode(out.x), look_encode(out.y), look_encode(out.z));
        if p(P_HDR_CURVES + 1u) != 0.0 {
            q = vec3(hdr_levels(q.x), hdr_levels(q.y), hdr_levels(q.z));
        }
        if offset(P_PARAMETRIC_LUT) >= 0 {
            q = encoded_tone(q, 1u);
        }
        q = encoded_tone(q, 2u);
        out = vec3(hdr_channel(0u, q.x), hdr_channel(1u, q.y), hdr_channel(2u, q.z));
    }
    return out;
}
// color_mixer::lookup_with on a grid of changes (three floats a cell) at HSV `h`, `s`,
// `v`, or of value factors (`scalar`, one float a cell, in the third component).
fn mixer_lookup(base: i32, h: f32, s: f32, v: f32, scalar: bool) -> vec3<f32> {
    let fh = rem_euclid(h, 1.0) * 36.0 - 0.5;
    let fs = clamp(sqrt(clamp(s, 0.0, 1.0)) * 6.0 - 0.5, 0.0, 5.0);
    let fv = clamp(powf(clamp(v, 0.0, 1.0), 0.45) * 6.0 - 0.5, 0.0, 5.0);
    let h0 = floor(fh);
    let th = fh - h0;
    let ts = fract(fs);
    let tv = fract(fv);
    let s0 = u32(fs);
    let v0 = u32(fv);
    let s1 = min(s0 + 1u, 5u);
    let v1 = min(v0 + 1u, 5u);
    var out = vec3(0.0);
    for (var dh = 0; dh < 2; dh++) {
        let hi = u32(rem_euclid(h0 + f32(dh), 36.0));
        let wh = select(1.0 - th, th, dh == 1);
        for (var k = 0u; k < 2u; k++) {
            let si = select(s0, s1, k == 1u);
            let ws = select(1.0 - ts, ts, k == 1u);
            for (var m = 0u; m < 2u; m++) {
                let vi = select(v0, v1, m == 1u);
                let wv = select(1.0 - tv, tv, m == 1u);
                let cell = i32((hi * 6u + si) * 6u + vi);
                if scalar {
                    out.z += table(base + cell) * (wh * ws * wv);
                } else {
                    out += table3(base + cell * 3) * (wh * ws * wv);
                }
            }
        }
    }
    return out;
}
// color_mixer::tables_in with `Lookup::Hdr`: the changes looked up at the colour's
// value up to HDR_MIX_VALUE, and the same above it; with `fading` (a grid of Vibrance's
// value factors, or -1) those factors at the colour's own value, faded out from
// HDR_MIX_FADE to white; with `luminance` (band Luminance's changes, or -1) those
// changes looked up at the colour's value up to HDR_LUMINANCE_VALUE, their value on each
// ProPhoto channel, faded out by its scene level (`scene`, as the other changes change
// it) from HDR_LUMINANCE_FADE to white. Linear sRGB primaries in and out.
fn mixer_hdr(rgb: vec3<f32>, base: i32, fading: i32, luminance: i32, scene: vec3<f32>) -> vec3<f32> {
    let q = max(RGB_TO_PRO * rgb, vec3(0.0));
    let max_v = max(max(max(q.x, q.y), q.z), 0.0);
    let min_v = min(min(q.x, q.y), q.z);
    if max_v <= 1e-6 {
        return rgb;
    }
    let d = max_v - min_v;
    let h = hue_of(q, max_v, d);
    let s = d / max_v;
    let low = min(max_v, HDR_MIX_VALUE);
    var delta = mixer_lookup(base, h, s, low, false);
    if fading >= 0 {
        let fade = clamp((1.0 - max_v) / (1.0 - HDR_MIX_FADE), 0.0, 1.0);
        delta.z = delta.z - mixer_lookup(fading, h, s, low, true).z
            + fade * mixer_lookup(fading, h, s, max_v, true).z;
    }
    var lum = 0.0;
    if luminance >= 0 {
        // Band Luminance's whole change at its own lookup value; its value factor
        // applies per channel below.
        let within = mixer_lookup(luminance, h, s, low, false);
        let own = mixer_lookup(luminance, h, s, min(max_v, HDR_LUMINANCE_VALUE), false);
        let keep = 1.0 - clamp((max_v - HDR_LUMINANCE_VALUE) / (1.0 - HDR_LUMINANCE_VALUE), 0.0, 1.0);
        delta = vec3(delta.x + keep * (own.x - within.x), delta.y + keep * (own.y - within.y), delta.z - within.z);
        lum = own.z;
    }
    let h2 = rem_euclid(h + delta.x, 1.0) * 6.0;
    let s2 = clamp(s * exp2(delta.y), 0.0, 1.0);
    let v = max_v * exp2(delta.z);
    let c = v * s2;
    var out = hsv_to_rgb(h2, v, s2) + (v - c);
    if lum != 0.0 {
        // Band Luminance by each channel's scene level as the other changes change
        // it, none from white up.
        let level = select(out, max(scene, vec3(0.0)) * out / max(q, vec3(1e-9)), q > vec3(1e-9));
        let w = clamp((vec3(1.0) - level) / (1.0 - HDR_LUMINANCE_FADE), vec3(0.0), vec3(1.0));
        out = out * exp2(lum * w);
    }
    return PRO_TO_RGB * out;
}
// HdrFinish::finish's colour stage: the colour mixer (with Saturation's fade to the
// gray source's luminance), colour grading, then the black & white mix's gray,
// unclipped above white.
// Linear ProPhoto, not negative, in and out.
fn hdr_colour_stage(pro: vec3<f32>, scene: vec3<f32>) -> vec3<f32> {
    let grid = offset(P_MIXER);
    let gray = offset(P_GRAY_GRID);
    let table = offset(P_RGB) >= 0 && p(P_RGB + 5u) != 0.0;
    let graded = offset(P_GRADE) >= 0;
    if grid < 0 && gray < 0 && !table && !graded {
        return pro;
    }
    var rgb = PRO_TO_RGB * pro;
    if grid >= 0 {
        var mixed = mixer_hdr(rgb, grid, offset(P_HDR_MIXER), offset(P_HDR_MIXER + 2u), scene);
        let fade = p(P_SATURATION_GRAY);
        if fade != 0.0 {
            var source = rgb;
            if offset(P_GRAY_SOURCE) >= 0 {
                source = mixer_hdr(rgb, offset(P_GRAY_SOURCE), offset(P_HDR_MIXER + 1u), offset(P_HDR_MIXER + 3u), scene);
            }
            let y = dot(source, vec3(0.2126, 0.7152, 0.0722));
            mixed = mixed + fade * (y - mixed);
        }
        rgb = mixed;
    }
    if table {
        rgb = rgb_table_hdr(rgb);
    }
    // Colour grading follows the colour mixer and a look's RGB table, as in SDR.
    if graded {
        rgb = PRO_TO_RGB * grade_hdr(RGB_TO_PRO * rgb);
    }
    if gray >= 0 {
        let scaled = mixer_hdr(max(rgb, vec3(0.0)), gray, -1, -1, vec3(0.0));
        rgb = vec3(max(dot(scaled, vec3(0.2126, 0.7152, 0.0722)), 0.0));
    }
    return max(RGB_TO_PRO * rgb, vec3(0.0));
}
// The HDR tone curve (`HdrTone::evaluate`) from its samples over log2 scene value
// (`pixel_params::hdr_tone_table`): log2–log2 interpolation, linear below the first
// sample, and the last samples' slope above the last.
fn hdr_curve(x: f32) -> f32 {
    if !(x > 0.0) {
        return 0.0;
    }
    let base = offset(P_HDR + 1u);
    let n = HDR_TONE.z;
    let f = (log2(x) - HDR_TONE.x) / HDR_TONE.y;
    if f <= 0.0 {
        return x * exp2(table(base) - HDR_TONE.x);
    }
    if f >= n - 1.0 {
        let a = table(base + i32(n) - 2);
        let b = table(base + i32(n) - 1);
        return exp2(b + (b - a) * (f - (n - 1.0)));
    }
    let i = floor(f);
    return exp2(mix(table(base + i32(i)), table(base + i32(i) + 1), f - i));
}
// CameraProfile::finish_hdr, then `HdrFinish::finish` without what follows the
// profile: the profile's look table (on overrange-encoded values for an HDR profile),
// a look's table (always so), the HDR tone curve DNG RGBTone fashion (an HDR profile's
// own on encoded values, else the measured extension), then the look's curve; linear
// ProPhoto in and out, not negative.
fn hdr_finish(pro: vec3<f32>) -> vec3<f32> {
    let hdr_profile = p(P_HDR_PROFILE) != 0.0;
    var q = max(pro, vec3(0.0));
    if offset(P_LOOK) >= 0 {
        if hdr_profile {
            q = table_apply_overrange(table_at(P_LOOK), q, -1, 0.0);
        } else {
            q = table_apply(table_at(P_LOOK), q, -1, 0.0);
        }
    }
    if offset(P_ENH) >= 0 {
        q = table_apply_overrange(table_at(P_ENH), q, -1, 0.0);
    }
    q = max(q, vec3(0.0));
    if hdr_profile {
        q = vec3(overrange_encode(q.x), overrange_encode(q.y), overrange_encode(q.z));
    }
    let low = min(min(q.x, q.y), q.z);
    let high = max(max(q.x, q.y), q.z);
    var a: f32;
    var b: f32;
    if hdr_profile {
        a = overrange_tone(low);
        b = overrange_tone(high);
    } else {
        a = hdr_curve(low);
        b = hdr_curve(high);
    }
    var out = vec3(b);
    if high - low > 1e-8 {
        out = a + (b - a) * (q - low) / (high - low);
    }
    if hdr_profile {
        out = vec3(overrange_decode(out.x), overrange_decode(out.y), overrange_decode(out.z));
    }
    if p(P_HDR_LOOK) != 0.0 {
        out = look_apply(out);
    }
    // Contrast and the tone curves follow the profile and its look, and the colour
    // stage follows them.
    return hdr_colour_stage(max(hdr_tone_controls(max(out, vec3(0.0))), vec3(0.0)), pro);
}
// CameraProfile::finish: linear ProPhoto in, linear sRGB primaries out.
fn profile_finish(pro: vec3<f32>) -> vec3<f32> {
    var q = pro;
    if offset(P_LOOK) >= 0 {
        q = table_apply(table_at(P_LOOK), q, -1, 0.0);
    }
    if offset(P_ENH) >= 0 {
        q = table_apply(table_at(P_ENH), clamp(q, vec3(0.0), vec3(1.0)), -1, 0.0);
    }
    q = clamp(q, vec3(0.0), vec3(1.0));
    let low = min(min(q.x, q.y), q.z);
    let high = max(max(max(q.x, q.y), q.z), 0.0);
    q = rgb_tone_values(q, tone_eval(low), tone_eval(high), low, high);
    if offset(P_ENH_CURVE) >= 0 {
        q = enhanced_curve(q);
    }
    return PRO_TO_RGB * q;
}
fn calibrate(color: vec3<f32>) -> vec3<f32> {
    let q = matrix(P_CALIBRATION) * color;
    let shadow = p(P_SHADOW_TINT);
    if shadow == 0.0 {
        return q;
    }
    let y = max(0.2126 * q.x + 0.7152 * q.y + 0.0722 * q.z, 0.0);
    let amount = abs(shadow) * y * exp(-6.0 * y);
    let direction = select(vec3(-0.331, 0.029, -0.152), vec3(0.116, -0.189, -0.002), shadow > 0.0);
    return q + amount * direction;
}
// scene_tone::global::evaluate: a curve sampled at log2 inputs `first` + k·`step`,
// linear below its first sample and constant above its last.
fn log_curve(base: i32, grid: vec3<f32>, x: f32) -> f32 {
    let first = grid.x;
    let step = grid.y;
    let n = u32(grid.z);
    let low = exp2(first);
    if x <= low {
        return x * table(base) / low;
    }
    let f = (log2(x) - first) / step;
    if f >= f32(n - 1u) {
        return table(base + i32(n) - 1);
    }
    let i = u32(f);
    let t = f - f32(i);
    return mix(table(base + i32(i)), table(base + i32(i) + 1), t);
}
// scene_tone::global::bracket on a regular grid (first, step, count).
fn grid_bracket(v: f32, grid: vec3<f32>) -> vec2<f32> {
    let f = clamp((v - grid.x) / grid.y, 0.0, grid.z - 1.0);
    let i = min(floor(f), grid.z - 2.0);
    return vec2(i, f - i);
}
// GlobalTone::white_at at Whites `whites` (`gpu_curves` layout: the rows, which carry
// the toe's removal without a black point, then the tail).
fn white_curve(whites: f32, x_in: f32) -> f32 {
    // Without a black point the curve continues linearly below `linear_below`.
    let linear_below = p(P_SCENE_KEYS + 5u);
    let below = p(P_SCENE_KEYS + 2u) == 0.0 && x_in > 0.0 && x_in < linear_below;
    let x = select(x_in, linear_below, below);
    let rows = i32(SCENE_U.z);
    let base = offset(P_SCENE_WHITE);
    let b = grid_bracket(whites, SCENE_S);
    let r0 = base + i32(b.x) * rows;
    let r1 = r0 + rows;
    let tail = base + i32(SCENE_S.z) * rows;
    let top = p(P_SCENE_KEYS);
    let top_x = exp2(top);
    var y = 0.0;
    if x <= top_x {
        let grid = vec3(top + SCENE_U.x, SCENE_U.y, SCENE_U.z);
        y = mix(log_curve(r0, grid, x), log_curve(r1, grid, x), b.y);
    } else {
        let at_top = mix(table(r0 + rows - 1), table(r1 + rows - 1), b.y);
        let r = log_curve(tail, SCENE_T, x);
        let rm = log_curve(tail, SCENE_T, top_x);
        y = at_top;
        // A curve Whites has already stretched past white stays there (global.rs).
        if rm < 1.0 && at_top < 1.0 {
            y = at_top + (1.0 - at_top) * clamp((r - rm) / (1.0 - rm), 0.0, 1.0);
        }
    }
    if below {
        y = y * x_in / linear_below;
    }
    return y;
}
// hdr_whites::gain: an HDR render's Whites gain (log2) at scene value `x`.
fn hdr_gain(x: f32) -> f32 {
    let base = offset(P_HDR);
    let n = SCENE_H.z;
    if !(x > 0.0) {
        return table(base);
    }
    let f = clamp((log2(x) - SCENE_H.x) / SCENE_H.y, 0.0, n - 1.0);
    let i = min(floor(f), n - 2.0);
    return mix(table(base + i32(i)), table(base + i32(i) + 1), f - i);
}
// GlobalTone::white_at in an HDR render: the identity from HDR_IDENTITY_FROM up, the
// default black point's toe below it (the white curve at Whites 0, which `hdr` builds
// the rows at), each times the Whites gain.
fn hdr_white(x: f32) -> f32 {
    let toe = select(white_curve(0.0, x), x, x >= HDR_IDENTITY_FROM);
    return toe * exp2(hdr_gain(x));
}
// global::mask_curve: a mask's Whites (`kind` 0) or Blacks (1) at `s` on output `y`.
fn mask_row(kind: u32, row: u32, y: f32) -> f32 {
    let n = u32(SCENE_Y.z);
    let base = offset(P_SCENE_TABLES) + i32((kind * 8u + row) * n);
    return log_curve(base, SCENE_Y, y);
}
fn mask_position(slot: u32) -> f32 {
    if slot == 4u {
        return 0.0;
    }
    return MASK_VALUES[select(slot - 1u, slot, slot < 4u)];
}
fn mask_output(kind: u32, slot: u32, y: f32) -> f32 {
    if slot == 4u {
        return y;
    }
    return mask_row(kind, select(slot - 1u, slot, slot < 4u), y);
}
fn mask_curve(kind: u32, s_in: f32, y: f32) -> f32 {
    if s_in == 0.0 {
        return y;
    }
    let s = clamp(s_in, -1.0, 1.0);
    var j = 7u;
    for (var k = 0u; k < 8u; k++) {
        if s <= mask_position(k + 1u) {
            j = k;
            break;
        }
    }
    let t = (s - mask_position(j)) / (mask_position(j + 1u) - mask_position(j));
    return mix(mask_output(kind, j, y), mask_output(kind, j + 1u, y), t);
}
// GlobalTone::apply's default black on one channel: identity at and above white.
fn default_black(base: i32, y: f32) -> f32 {
    if y >= 1.0 {
        return y;
    }
    return log_curve(base, SCENE_Y, y);
}
// GlobalTone::apply: the white curve (RGBTone), then the default black and Blacks per
// channel.
fn scene_global(q: vec3<f32>) -> vec3<f32> {
    let whites = p(P_SCENE_KEYS + 3u);
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(q.x, q.y), q.z);
    var ya: f32;
    var yb: f32;
    if V_HDR {
        ya = hdr_white(lo);
        yb = hdr_white(hi);
    } else {
        ya = white_curve(whites, lo);
        yb = white_curve(whites, hi);
    }
    var out = vec3(ya);
    if hi - lo > 1e-12 {
        out = ya + (yb - ya) * (q - lo) / (hi - lo);
    }
    let black = offset(P_SCENE_DEFAULT_BLACK);
    let shadow_black = offset(P_SCENE_SHADOW_BLACK);
    if shadow_black >= 0 && (V_MASKS && masked) && delta[L_SHADOWS] != 0.0 {
        let f = clamp(p(P_SCENE_SHADOW_BLACK + 1u) + delta[L_SHADOWS], 0.0, 1.0) * 10.0;
        let i = min(i32(f), 9);
        let a = shadow_black + i * i32(SCENE_Y.z);
        let b = a + i32(SCENE_Y.z);
        out = mix(vec3(default_black(a, out.x), default_black(a, out.y), default_black(a, out.z)),
                  vec3(default_black(b, out.x), default_black(b, out.y), default_black(b, out.z)), f - f32(i));
    } else if black >= 0 {
        out = vec3(default_black(black, out.x), default_black(black, out.y), default_black(black, out.z));
    }
    let blacks = offset(P_SCENE_BLACKS);
    if blacks >= 0 {
        let curved = vec3(log_curve(blacks, SCENE_Y, out.x), log_curve(blacks, SCENE_Y, out.y), log_curve(blacks, SCENE_Y, out.z));
        // In HDR, Blacks leaves values above white alone (`GlobalTone::apply`).
        if V_HDR {
            out = select(curved, out, out >= vec3(1.0));
        } else {
            out = curved;
        }
    }
    return out;
}
// GlobalTone::apply_at: a mask's Whites (RGBTone) and Blacks (per channel) after the
// global curves.
fn scene_mask(q: vec3<f32>, whites: f32, blacks: f32) -> vec3<f32> {
    var out = q;
    if whites != 0.0 {
        let lo = min(min(q.x, q.y), q.z);
        let hi = max(max(q.x, q.y), q.z);
        let ya = mask_curve(0u, whites, lo);
        let yb = mask_curve(0u, whites, hi);
        out = vec3(ya);
        if hi - lo > 1e-12 {
            out = ya + (yb - ya) * (q - lo) / (hi - lo);
        }
    }
    if blacks != 0.0 {
        out = vec3(mask_curve(1u, blacks, out.x), mask_curve(1u, blacks, out.y), mask_curve(1u, blacks, out.z));
    }
    return out;
}
// local_tone::LocalToneMap::gain
fn local_curve(i: u32, base: f32, tail_base: f32) -> f32 {
    let t = offset(i);
    if t < 0 {
        return 0.0;
    }
    let key = p(i + 1u);
    let lo = p(i + 2u);
    let hi = p(i + 3u);
    let f = clamp((base - key - lo) / (hi - lo) * 48.0 - 0.5, 0.0, 47.0);
    let j = min(u32(f), 46u);
    let a = table(t + i32(j));
    // The tail per stop past the last column follows the table (`Curve::tail`).
    let past = max(tail_base - key - (hi - (hi - lo) / 96.0), 0.0);
    return a + (table(t + i32(j) + 1) - a) * (f - f32(j)) + table(t + 48) * past;
}
// Slider position `k` of the 7 around a measured family, with 0 inserted at index 3,
// and its table index (-1 for the identity).
fn slider_point(values: i32, k: u32) -> f32 {
    if k == 3u {
        return 0.0;
    }
    return table(values + i32(select(k - 1u, k, k < 3u)));
}
fn slider_table(k: u32) -> i32 {
    if k == 3u {
        return -1;
    }
    return i32(select(k - 1u, k, k < 3u));
}
// The bracketing pair of measured positions for slider `s` and the weight between.
fn bracket(values: i32, s: f32) -> vec2<f32> {
    var j = 5u;
    for (var k = 0u; k < 6u; k++) {
        if s <= slider_point(values, k + 1u) {
            j = k;
            break;
        }
    }
    let s0 = slider_point(values, j);
    let s1 = slider_point(values, j + 1u);
    return vec2(f32(j), (s - s0) / (s1 - s0));
}
// local_tone::family: a Shadows (0), Highlights (1), Dehaze (2), Shadows detail (3) or
// Shadows region (4) gain at slider `s`
// (layout: `local_tone::gpu_families`).
fn family(f: u32, s_in: f32, key: f32, base: f32, tail_base: f32) -> f32 {
    if s_in == 0.0 {
        return 0.0;
    }
    let s = clamp(s_in, -1.0, 1.0);
    let t = offset(P_LOCAL_FAMILIES) + i32(f * 302u);
    let lo = table(t + 288);
    let hi = table(t + 289);
    let b = bracket(t + 290, s);
    let j = u32(b.x);
    let x = clamp((base - key - lo) / (hi - lo) * 48.0 - 0.5, 0.0, 47.0);
    let i = min(u32(x), 46u);
    let past = max(tail_base - key - (hi - (hi - lo) / 96.0), 0.0);
    var y = vec2(0.0);
    for (var k = 0u; k < 2u; k++) {
        let row = slider_table(j + k);
        if row >= 0 {
            let a = table(t + row * 48 + i32(i));
            y[k] = a + (table(t + row * 48 + i32(i) + 1) - a) * (x - f32(i)) + table(t + 296 + row) * past;
        }
    }
    return y.x + (y.y - y.x) * b.y;
}
// basic_tone::slider over a 6 × 64 table at `t`, with positions at `values`.
fn measured(t: i32, values: i32, s_in: f32, x: f32) -> f32 {
    if s_in == 0.0 {
        return x;
    }
    let s = clamp(s_in, -1.0, 1.0);
    let b = bracket(values, s);
    let j = u32(b.x);
    let f = clamp(x * 64.0 - 0.5, -0.5, 63.5);
    let i = clamp(i32(floor(f)), 0, 62);
    var y = vec2(x);
    for (var k = 0u; k < 2u; k++) {
        let row = slider_table(j + k);
        if row >= 0 {
            let a = table(t + row * 64 + i);
            y[k] = clamp(a + (table(t + row * 64 + i + 1) - a) * (f - f32(i)), 0.0, 1.0);
        }
    }
    return y.x + (y.y - y.x) * b.y;
}
// masks::local::tone's curve: Contrast at the pixel's local slider value (layout:
// `basic_tone::gpu_tables`).
fn local_tone_curve(x_in: f32) -> f32 {
    let t = offset(P_LOCAL_TONE);
    let x = x_in;
    let pivot = p(P_LOCAL_PIVOT);
    return contrast_at(t + 6, table(t + 390), pivot, t, delta[L_CONTRAST], x);
}
// basic_tone::contrast_at: the chart's Contrast table at `t` (pivoting at `chart`)
// moved to `pivot` by a power warp of gamma-2.2 encoded values.
fn contrast_at(t: i32, chart: f32, pivot: f32, values: i32, s: f32, x: f32) -> f32 {
    if s == 0.0 {
        return x;
    }
    let k = log(gamma22(chart)) / log(gamma22(pivot));
    let y = measured(t, values, s, from_gamma22(powf(gamma22(x), k)));
    return from_gamma22(powf(gamma22(y), 1.0 / k));
}
fn gamma22(v: f32) -> f32 {
    return powf(srgb_decode(clamp(v, 0.0, 1.0)), 1.0 / 2.2);
}
fn from_gamma22(w: f32) -> f32 {
    return srgb_encode(powf(clamp(w, 0.0, 1.0), 2.2));
}
fn local_gain(pos: vec2<f32>, lum: f32, clarity_lum: f32) -> f32 {
    let w = u32(p(P_LOCAL_SIZE));
    let h = u32(p(P_LOCAL_SIZE + 1u));
    let fx = clamp((pos.x + 0.5) * p(P_LOCAL_SCALE) - 0.5, 0.0, f32(w - 1u));
    let fy = clamp((pos.y + 0.5) * p(P_LOCAL_SCALE + 1u) - 0.5, 0.0, f32(h - 1u));
    let ix = u32(fx);
    let iy = u32(fy);
    let jx = min(ix + 1u, w - 1u);
    let jy = min(iy + 1u, h - 1u);
    let tx = fx - f32(ix);
    let ty = fy - f32(iy);
    var coef = vec2(0.0);
    for (var k = 0u; k < 2u; k++) {
        let base = u32(offset(P_LOCAL_A + k));
        let top = grid[base + iy * w + ix] * (1.0 - tx) + grid[base + iy * w + jx] * tx;
        let bottom = grid[base + jy * w + ix] * (1.0 - tx) + grid[base + jy * w + jx] * tx;
        coef[k] = top * (1.0 - ty) + bottom * ty;
    }
    let base = coef.x * log2(max(lum, LOCAL_FLOOR)) + coef.y;
    // Highlights' tail reads the base, but no more than 0.5 stops above the fine mean
    // (`LocalToneMap::tail_base`).
    let f = u32(offset(P_LOCAL_FINE));
    let fine_top = grid[f + iy * w + ix] * (1.0 - tx) + grid[f + iy * w + jx] * tx;
    let fine_bottom = grid[f + jy * w + ix] * (1.0 - tx) + grid[f + jy * w + jx] * tx;
    let tail_base = min(base, fine_top * (1.0 - ty) + fine_bottom * ty + 0.5);
    // Joint bilateral upsampling of the calibrated Clarity gains. Fine luminance
    // selects neighbouring gains without extrapolating new contrast.
    var clarity = 0.0;
    let c = offset(P_LOCAL_A + 2u);
    if c >= 0 {
        let indices = vec4(iy * w + ix, iy * w + jx, jy * w + ix, jy * w + jx);
        let spatial = vec4((1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty);
        let log_lum = log2(max(clarity_lum, LOCAL_FLOOR));
        var gains = vec4(0.0);
        var weights = vec4(0.0);
        for (var k = 0u; k < 4u; k++) {
            gains[k] = table(c + i32(indices[k]));
            if spatial[k] > 0.0 {
                let guide = table(c + i32(w * h + indices[k]));
                let d = (guide - log_lum) / 0.25;
                weights[k] = spatial[k] / (1.0 + d * d);
            }
        }
        clarity = dot(gains, weights) / dot(weights, vec4(1.0));
    }
    // Positive Shadows' regional level (local_tone::LocalToneMap::lift), on the same grid.
    var region = 0.0;
    let r = offset(P_LOCAL_REGION);
    if r >= 0 {
        let g = u32(r);
        let top = grid[g + iy * w + ix] * (1.0 - tx) + grid[g + iy * w + jx] * tx;
        let bottom = grid[g + jy * w + ix] * (1.0 - tx) + grid[g + jy * w + jx] * tx;
        region = top * (1.0 - ty) + bottom * ty;
    }
    if (V_MASKS && masked) && (delta[L_SHADOWS] != 0.0 || delta[L_HIGHLIGHTS] != 0.0) {
        let s = p(P_GLOBAL_SH) + delta[L_SHADOWS];
        let h = p(P_GLOBAL_SH + 1u) + delta[L_HIGHLIGHTS];
        var lift = 0.0;
        if r >= 0 {
            lift = family(3u, s, 0.0, base - region, base - region) + family(4u, s, p(P_LOCAL_KEYS + 2u), region, region);
        }
        // Positive Shadows and negative Highlights follow the photo's global rule
        // (`local_tone_rule`), scaled by the slider; the other signs their families.
        var sg = family(0u, s, p(P_LOCAL_KEYS), base, base);
        if s > 0.0 && offset(P_RULE) >= 0 {
            sg = local_curve(P_RULE, base, base) * min(s, 1.0);
        }
        var hg = family(1u, h, p(P_LOCAL_KEYS + 1u), base, tail_base);
        if h < 0.0 && offset(P_RULE + 4u) >= 0 {
            hg = local_curve(P_RULE + 4u, base, base) * min(-h, 1.0);
        }
        return exp2(sg + hg + lift + clarity);
    }
    var lift = 0.0;
    if r >= 0 {
        lift = local_curve(P_LIFT, base - region, base - region) + local_curve(P_LIFT + 4u, region, region);
    }
    return exp2(local_curve(P_SHADOWS, base, base) + local_curve(P_HIGHLIGHTS, base, tail_base) + lift + clarity);
}
// scene_tone::haze: the fraction of the haze positive Dehaze `s` removes.
fn haze_omega(s_in: f32) -> f32 {
    let s = clamp(s_in, 0.0, 1.0);
    if s <= 0.2 {
        return 1.5 * s;
    }
    if s <= 0.4 {
        return 0.3 + 0.75 * (s - 0.2);
    }
    return 0.45 + 0.75 * (s - 0.4);
}
// Dehazing::apply: the photo's haze removed at Dehaze `s` > 0.
fn dehaze_positive(pro: vec3<f32>, s: f32, pos: vec2<f32>, tone_gain: f32) -> vec3<f32> {
    let base = offset(P_HAZE);
    if base < 0 {
        return pro;
    }
    let w = u32(p(P_HAZE + 1u));
    let h = u32(p(P_HAZE + 2u));
    let fx = clamp((pos.x + 0.5) * p(P_HAZE + 3u) - 0.5, 0.0, f32(w - 1u));
    let fy = clamp((pos.y + 0.5) * p(P_HAZE + 4u) - 0.5, 0.0, f32(h - 1u));
    let ix = u32(fx);
    let iy = u32(fy);
    let jx = min(ix + 1u, w - 1u);
    let jy = min(iy + 1u, h - 1u);
    let tx = fx - f32(ix);
    let ty = fy - f32(iy);
    let top = table(base + i32(iy * w + ix)) * (1.0 - tx) + table(base + i32(iy * w + jx)) * tx;
    let bottom = table(base + i32(jy * w + ix)) * (1.0 - tx) + table(base + i32(jy * w + jx)) * tx;
    let density = top * (1.0 - ty) + bottom * ty;
    let t = max(1.0 - haze_omega(s) * density, 0.3);
    var gain = tone_gain;
    if (V_MASKS && masked) { gain *= exp2(delta[L_EXPOSURE]); }
    let air = vec3(p(P_HAZE_AIR), p(P_HAZE_AIR + 1u), p(P_HAZE_AIR + 2u)) * gain;
    let j = (pro - air) / t + air;
    // haze.rs: no channel below half the colour scaled by the luminance's change.
    let lw = vec3(0.2880402, 0.7118741, 0.0000857);
    let ratio = max(dot(j, lw), 0.0) / max(dot(pro, lw), 1e-9);
    return max(j, 0.5 * pro * ratio);
}
fn level(v: f32) -> f32 {
    let bp = p(P_LEVELS);
    let wp = p(P_LEVELS + 1u);
    let midtone = p(P_LEVELS + 2u);
    var x = clamp((v - bp) / (wp - bp), 0.0, 1.0);
    if midtone != 1.0 {
        x = powf(x, 1.0 / midtone);
    }
    // Contrast is a measured curve on this path: the S-curve has power 1.
    let low = x;
    return low / max(low + (1.0 - x), 1e-8);
}
// curve::refine_saturation: Refine Saturation below 100 keeps the colour's channel
// differences from before the point curve, with the curved colour's luma.
fn refine_saturation(input: vec3<f32>, curved: vec3<f32>) -> vec3<f32> {
    let amount = p(P_REFINE_SATURATION);
    if amount >= 1.0 {
        return curved;
    }
    let luma = vec3(0.299, 0.587, 0.114);
    let goal = dot(curved, luma);
    let offset = input - vec3(dot(input, luma));
    let high = max(max(max(offset.x, offset.y), offset.z), 0.0);
    let low = min(min(min(offset.x, offset.y), offset.z), 0.0);
    var scale = 1.0;
    if high > 1e-6 {
        scale = min(scale, (1.0 - goal) / high);
    }
    if low < -1e-6 {
        scale = min(scale, goal / -low);
    }
    let kept = goal + offset * max(scale, 0.0);
    return kept + (curved - kept) * amount;
}
fn reference_curves(rgb: vec3<f32>) -> vec3<f32> {
    let pro = RGB_TO_PRO * rgb;
    var q = vec3(
        srgb_encode(clamp(pro.x, 0.0, 1.0)),
        srgb_encode(clamp(pro.y, 0.0, 1.0)),
        srgb_encode(clamp(pro.z, 0.0, 1.0)),
    );
    let basic = offset(P_BASIC);
    if basic >= 0 {
        q = clamp(q, vec3(0.0), vec3(1.0));
        let lo = min(min(q.x, q.y), q.z);
        let hi = max(max(max(q.x, q.y), q.z), 0.0);
        q = rgb_tone_values(q, lut(basic, 1024u, lo), lut(basic, 1024u, hi), lo, hi);
    }
    if (V_MASKS && masked) && delta[L_CONTRAST] != 0.0 {
        q = clamp(q, vec3(0.0), vec3(1.0));
        let lo = min(min(q.x, q.y), q.z);
        let hi = max(max(max(q.x, q.y), q.z), 0.0);
        q = rgb_tone_values(q, local_tone_curve(lo), local_tone_curve(hi), lo, hi);
    }
    q = vec3(level(q.x), level(q.y), level(q.z));
    let parametric = offset(P_PARAMETRIC_LUT);
    if parametric >= 0 {
        let lo = min(min(q.x, q.y), q.z);
        let hi = max(max(max(q.x, q.y), q.z), 0.0);
        q = rgb_tone_values(q, lut(parametric, 1024u, lo), lut(parametric, 1024u, hi), lo, hi);
    }
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(max(q.x, q.y), q.z), 0.0);
    let master = offset(P_MASTER);
    let a = lut(master, 4096u, lo);
    let b = lut(master, 4096u, hi);
    var m = vec3(a);
    if hi - lo > 1e-8 {
        m = a + (b - a) * (q - lo) / (hi - lo);
    }
    m = refine_saturation(q, m);
    let channels = vec3(
        srgb_decode(lut(offset(P_CHANNELS), 4096u, m.x)),
        srgb_decode(lut(offset(P_CHANNELS + 1u), 4096u, m.y)),
        srgb_decode(lut(offset(P_CHANNELS + 2u), 4096u, m.z)),
    );
    return PRO_TO_RGB * channels;
}
// color_mixer::ColorMixer (36 hues × 6 saturations × 6 values).
fn mixer(rgb: vec3<f32>, base: i32) -> vec3<f32> {
    let q = max(RGB_TO_PRO * rgb, vec3(0.0));
    let max_v = max(max(max(q.x, q.y), q.z), 0.0);
    let min_v = min(min(q.x, q.y), q.z);
    if max_v <= 1e-6 {
        return rgb;
    }
    let d = max_v - min_v;
    let h = hue_of(q, max_v, d);
    let s = d / max_v;
    let fh = rem_euclid(h, 1.0) * 36.0 - 0.5;
    let fs = clamp(sqrt(clamp(s, 0.0, 1.0)) * 6.0 - 0.5, 0.0, 5.0);
    let fv = clamp(powf(clamp(max_v, 0.0, 1.0), 0.45) * 6.0 - 0.5, 0.0, 5.0);
    let h0 = floor(fh);
    let th = fh - h0;
    let ts = fract(fs);
    let tv = fract(fv);
    let s0 = u32(fs);
    let v0 = u32(fv);
    let s1 = min(s0 + 1u, 5u);
    let v1 = min(v0 + 1u, 5u);
    var delta = vec3(0.0);
    for (var dh = 0; dh < 2; dh++) {
        let hi = u32(rem_euclid(h0 + f32(dh), 36.0));
        let wh = select(1.0 - th, th, dh == 1);
        for (var k = 0u; k < 2u; k++) {
            let si = select(s0, s1, k == 1u);
            let ws = select(1.0 - ts, ts, k == 1u);
            for (var m = 0u; m < 2u; m++) {
                let vi = select(v0, v1, m == 1u);
                let wv = select(1.0 - tv, tv, m == 1u);
                delta += table3(base + i32((hi * 6u + si) * 6u + vi) * 3) * (wh * ws * wv);
            }
        }
    }
    let h2 = rem_euclid(h + delta.x, 1.0) * 6.0;
    let s2 = clamp(s * exp2(delta.y), 0.0, 1.0);
    let v = max_v * exp2(delta.z);
    let c = v * s2;
    return PRO_TO_RGB * (hsv_to_rgb(h2, v, s2) + (max_v * exp2(delta.z) - c));
}
// point_color::PointColors, in HSV of linear ProPhoto RGB with hue in radians.
fn smoothstep01(t: f32) -> f32 {
    return t * t * (3.0 - 2.0 * t);
}
fn point_ramp(x: f32, at: i32, rise: f32, fall: f32) -> f32 {
    let a = table(at);
    let b = table(at + 1);
    let c = table(at + 2);
    let d = table(at + 3);
    var up = 1.0;
    if x < b {
        up = select(powf(smoothstep01((x - a) / (b - a)), rise), 0.0, x <= a);
    }
    var down = 1.0;
    if x > c {
        down = select(powf(smoothstep01((d - x) / (d - c)), fall), 0.0, x >= d);
    }
    return min(up, down);
}
// Visualize Range's selection of this pixel, or -1 without it.
var<private> point_selection: f32;
// point_color::visualize, on the finished color.
fn visualize(out: vec3<f32>) -> vec3<f32> {
    let y = 0.2126 * srgb_decode(out.x) + 0.7152 * srgb_decode(out.y) + 0.0722 * srgb_decode(out.z);
    let gray = srgb_encode(y);
    return vec3(gray) + (out - vec3(gray)) * point_selection;
}
// One swatch at table offset `w`, on linear ProPhoto RGB.
fn point_color(p0: vec3<f32>, w: i32, base: i32) -> vec3<f32> {
    let q = max(p0, vec3(0.0));
    let max_v = max(max(q.x, q.y), q.z);
    if max_v <= 1e-6 {
        if table(w + 22) != 0.0 {
            point_selection = 0.0;
        }
        return p0;
    }
    let min_v = min(min(q.x, q.y), q.z);
    let d0 = max_v - min_v;
    let h = hue_of(q, max_v, d0) * TAU;
    let s = d0 / max_v;
    let v = max_v;
    let ev = srgb_encode(min(v, 1.0));
    let hue_ramp = table(base);
    let hue = table(w);
    let sat = table(w + 1);
    let lum = table(w + 2);
    let d = rem_euclid(h - hue + 3.14159265358979323846, TAU) - 3.14159265358979323846;
    let weight = point_ramp(d * table(w + 3), w + 6, hue_ramp, hue_ramp)
        * point_ramp(sat + (s - sat) * table(w + 4), w + 10, table(base + 1), table(base + 2))
        * point_ramp(lum + (ev - lum) * table(w + 5), w + 14, table(base + 3), table(base + 4))
        * min(s / table(base + 5), 1.0);
    // Visualize Range: note the selection; the finished color is grayed by the rest.
    if table(w + 22) != 0.0 {
        point_selection = weight;
    }
    if weight <= 0.0 {
        return p0;
    }
    let variance = table(w + 21);
    let turn = weight * (table(w + 18) + variance * d);
    var log_s = weight * table(w + 19);
    var log_v = weight * table(w + 20);
    if variance != 0.0 {
        let s2 = clamp(sat + (s - sat) * exp2(table(base + 6) * variance), 1e-4, 1.0);
        let e2 = clamp(lum + (ev - lum) * exp2(table(base + 7) * variance), 1e-4, 1.0);
        log_s += weight * log2(s2 / max(s, 1e-4));
        log_v += weight * log2(srgb_decode(e2) / max(srgb_decode(ev), 1e-6));
    }
    let h2 = rem_euclid((h + turn) / TAU, 1.0) * 6.0;
    let s2 = clamp(s * exp2(log_s), 0.0, 1.0);
    let v2 = v * exp2(log_v);
    let out = hsv_to_rgb(h2, v2, s2) + (v2 - v2 * s2);
    return min(out, max(q, vec3(1.0))) + (p0 - q);
}
// Swatches apply in turn, each to the result of the ones before.
fn point_colors(rgb: vec3<f32>) -> vec3<f32> {
    let base = offset(P_POINT);
    var q = RGB_TO_PRO * rgb;
    for (var i = 0u; i < u32(p(P_POINT + 1u)); i++) {
        q = point_color(q, base + POINT_CONSTANTS + i32(i) * POINT_SWATCH, base);
    }
    return PRO_TO_RGB * q;
}
// camera_profiles::rgb_table: a look's RGB table, in its own primaries and encoding.
const REC2020_ALPHA: f32 = 1.0992968;
const REC2020_BETA: f32 = 0.01805397;
fn rec2020_encode(v: f32) -> f32 {
    if v < REC2020_BETA {
        return 4.5 * v;
    }
    return REC2020_ALPHA * powf(v, 0.45) - (REC2020_ALPHA - 1.0);
}
fn rec2020_decode(v: f32) -> f32 {
    if v < 4.5 * REC2020_BETA {
        return v / 4.5;
    }
    return powf((v + REC2020_ALPHA - 1.0) / REC2020_ALPHA, 1.0 / 0.45);
}
fn rgb_encode(v: f32, gamma: u32) -> f32 {
    switch gamma {
        case 1u: { return srgb_encode(v); }
        case 2u: { return powf(v, 1.0 / 1.8); }
        case 3u: { return powf(v, 1.0 / 2.2); }
        case 4u: { return rec2020_encode(v); }
        default: { return v; }
    }
}
fn rgb_decode(v: f32, gamma: u32) -> f32 {
    switch gamma {
        case 1u: { return srgb_decode(v); }
        case 2u: { return powf(v, 1.8); }
        case 3u: { return powf(v, 2.2); }
        case 4u: { return rec2020_decode(v); }
        default: { return v; }
    }
}
/// Tetrahedral interpolation in a 3D table of `n` divisions, as `rgb_table::tetrahedral`.
fn rgb_lookup_3d(base: i32, n: u32, q: vec3<f32>) -> vec3<f32> {
    let scaled = q * f32(n - 1u);
    let cell = min(vec3<u32>(scaled), vec3(n - 2u));
    let f = scaled - vec3<f32>(cell);
    let n_i = i32(n);
    let origin = base + 3 * ((i32(cell.x) * n_i + i32(cell.y)) * n_i + i32(cell.z));
    let dr = 3 * n_i * n_i;
    let dg = 3 * n_i;
    let db = 3;
    let c000 = table3(origin);
    let c111 = table3(origin + dr + dg + db);
    var a: vec3<f32>;
    var b: vec3<f32>;
    var w: vec3<f32>;
    if f.x > f.y {
        if f.y > f.z {
            a = table3(origin + dr);
            b = table3(origin + dr + dg);
            w = vec3(f.x, f.y, f.z);
        } else if f.x > f.z {
            a = table3(origin + dr);
            b = table3(origin + dr + db);
            w = vec3(f.x, f.z, f.y);
        } else {
            a = table3(origin + db);
            b = table3(origin + dr + db);
            w = vec3(f.z, f.x, f.y);
        }
    } else if f.z > f.y {
        a = table3(origin + db);
        b = table3(origin + dg + db);
        w = vec3(f.z, f.y, f.x);
    } else if f.z > f.x {
        a = table3(origin + dg);
        b = table3(origin + dg + db);
        w = vec3(f.y, f.z, f.x);
    } else {
        a = table3(origin + dg);
        b = table3(origin + dr + dg);
        w = vec3(f.y, f.x, f.z);
    }
    return c000 + w.x * (a - c000) + w.y * (b - a) + w.z * (c111 - b);
}
fn rgb_lookup_1d(base: i32, n: u32, q: vec3<f32>) -> vec3<f32> {
    var out: vec3<f32>;
    for (var k = 0; k < 3; k++) {
        let scaled = q[k] * f32(n - 1u);
        let i = min(u32(scaled), n - 2u);
        let lo = table(base + 3 * i32(i) + k);
        let hi = table(base + 3 * i32(i + 1u) + k);
        out[k] = lo + (hi - lo) * (scaled - f32(i));
    }
    return out;
}
fn rgb_table(rgb: vec3<f32>) -> vec3<f32> {
    return matrix(P_RGB_BACK) * rgb_table_in_space(matrix(P_RGB_INTO) * rgb);
}
// RgbTable::apply_hdr: each channel overrange-encoded for the table, what encodes
// above 1 added back to its output in that encoding.
fn rgb_table_hdr(rgb: vec3<f32>) -> vec3<f32> {
    let linear = matrix(P_RGB_INTO) * rgb;
    var e: vec3<f32>;
    for (var k = 0; k < 3; k++) {
        e[k] = sign(linear[k]) * overrange_encode(abs(linear[k]));
    }
    let over = max(e - vec3(1.0), vec3(0.0));
    let out = rgb_table_in_space(e - over) + over;
    var back: vec3<f32>;
    for (var k = 0; k < 3; k++) {
        back[k] = sign(out[k]) * overrange_decode(abs(out[k]));
    }
    return matrix(P_RGB_BACK) * back;
}
// The table on linear values in its own primaries (`RgbTable::apply_in_space`).
fn rgb_table_in_space(linear: vec3<f32>) -> vec3<f32> {
    let base = offset(P_RGB);
    let n = u32(p(P_RGB + 2u));
    let gamma = u32(p(P_RGB + 3u));
    let amount = p(P_RGB + 5u);
    var full: vec3<f32>;
    for (var k = 0; k < 3; k++) {
        full[k] = sign(linear[k]) * rgb_encode(abs(linear[k]), gamma);
    }
    let q = clamp(full, vec3(0.0), vec3(1.0));
    var looked: vec3<f32>;
    if p(P_RGB + 1u) == 1.0 {
        looked = rgb_lookup_1d(base, n, q);
    } else {
        looked = rgb_lookup_3d(base, n, q);
    }
    var out = q + amount * (looked - q);
    if p(P_RGB + 4u) != 0.0 {
        out += full - q;
    } else {
        out = clamp(out, vec3(0.0), vec3(1.0));
    }
    for (var k = 0; k < 3; k++) {
        out[k] = sign(out[k]) * rgb_decode(abs(out[k]), gamma);
    }
    return out;
}
// color_grade_curves::ChannelCurves::channel: channel `c` of linear ProPhoto RGB
// through its gain curve.
fn grade_channel(c: i32, v: f32) -> f32 {
    let bins = u32(p(P_GRADE + 1u));
    let base = offset(P_GRADE);
    let f = powf(clamp(v, 0.0, 1.0), 1.0 / 2.2) * f32(bins - 1u);
    let i = min(u32(f), bins - 2u);
    let t = f - f32(i);
    let g = table(base + i32(i) * 3 + c) * (1.0 - t) + table(base + i32(i + 1u) * 3 + c) * t;
    return v * g;
}
// ChannelCurves::apply_hdr: each channel graded in `to_hdr_domain`, whose 1 is
// HDR_GRADE_TOP; above it the gain at it. Linear ProPhoto, not negative, in and out.
const HDR_GRADE_TOP: f32 = 16.0;
fn grade_hdr_channel(c: i32, v: f32) -> f32 {
    let k = (HDR_GRADE_TOP - 1.0) / HDR_GRADE_TOP;
    let x = min(v, HDR_GRADE_TOP);
    let u = clamp(grade_channel(c, x / (1.0 + x * k)), 0.0, 1.0);
    let graded = u / (1.0 - u * k);
    if v <= HDR_GRADE_TOP {
        return graded;
    }
    return v * graded / HDR_GRADE_TOP;
}
fn grade_hdr(pro: vec3<f32>) -> vec3<f32> {
    let q = max(pro, vec3(0.0));
    return vec3(grade_hdr_channel(0, q.x), grade_hdr_channel(1, q.y), grade_hdr_channel(2, q.z));
}
// color_grade_curves::ChannelCurves: a gain curve per channel of linear ProPhoto RGB.
fn grade(rgb: vec3<f32>) -> vec3<f32> {
    let bins = u32(p(P_GRADE + 1u));
    let base = offset(P_GRADE);
    let q = RGB_TO_PRO * rgb;
    var out: vec3<f32>;
    for (var c = 0; c < 3; c++) {
        let f = powf(clamp(q[c], 0.0, 1.0), 1.0 / 2.2) * f32(bins - 1u);
        let i = min(u32(f), bins - 2u);
        let t = f - f32(i);
        let g = table(base + i32(i) * 3 + c) * (1.0 - t) + table(base + i32(i + 1u) * 3 + c) * t;
        out[c] = q[c] * g;
    }
    return PRO_TO_RGB * out;
}
fn srgb_to_lab(q: vec3<f32>) -> vec3<f32> {
    let a = vec3(
        0.41222146 * q.x + 0.53633255 * q.y + 0.051445995 * q.z,
        0.2119035 * q.x + 0.6806995 * q.y + 0.10739696 * q.z,
        0.08830246 * q.x + 0.28171885 * q.y + 0.6299787 * q.z,
    );
    let b = vec3(cbrt(a.x), cbrt(a.y), cbrt(a.z));
    return vec3(
        0.21045426 * b.x + 0.7936178 * b.y - 0.004072047 * b.z,
        1.9779985 * b.x - 2.4285922 * b.y + 0.4505937 * b.z,
        0.025904037 * b.x + 0.78277177 * b.y - 0.80867577 * b.z,
    );
}
fn lab_to_srgb(q: vec3<f32>) -> vec3<f32> {
    let a = vec3(
        q.x + 0.39633778 * q.y + 0.21580376 * q.z,
        q.x - 0.105561346 * q.y - 0.06385417 * q.z,
        q.x - 0.08948418 * q.y - 1.2914855 * q.z,
    );
    let b = a * a * a;
    return vec3(
        4.0767417 * b.x - 3.3077116 * b.y + 0.23096994 * b.z,
        -1.268438 * b.x + 2.6097574 * b.y - 0.3413194 * b.z,
        -0.0041960863 * b.x - 0.7034186 * b.y + 1.7076147 * b.z,
    );
}
// effects::Effects::defringe_color
fn defringe_step(x: f32) -> f32 {
    let t = clamp((x + 0.025) / 0.05, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}
fn defringe(lab_in: vec3<f32>, h: f32) -> vec3<f32> {
    var lab = lab_in;
    var centers = array<f32, 2>(0.875, 0.46);
    for (var i = 0u; i < 2u; i++) {
        let amount = p(P_DEFRINGE + i);
        if amount == 0.0 { continue; }
        let lo = (p(P_DEFRINGE_RANGES + i * 2u) - 0.5) * 0.5;
        let hi = (p(P_DEFRINGE_RANGES + i * 2u + 1u) - 0.5) * 0.5;
        let d = rem_euclid(h - centers[i] + 0.5, 1.0) - 0.5;
        let w = defringe_step(d - lo) * defringe_step(hi - d);
        let chroma = length(lab.yz);
        let strength = (1.0 - 0.45 * exp(-chroma / 0.09)) * (1.0 - exp(-amount * 20.0 / 2.5));
        let k = 1.0 - w * strength;
        lab.y *= k;
        lab.z *= k;
    }
    return lab;
}
/// Oklab color controls after the measured mixer: hue angle round trip, Defringe and
/// Monochrome.
fn adjust(lab_in: vec3<f32>) -> vec3<f32> {
    var lab = lab_in;
    let chroma = length(lab.yz);
    let hue = rem_euclid(atan2(lab.z, lab.y), TAU) / TAU;
    let angle = hue * TAU;
    lab.x = clamp(lab.x, 0.0, 1.0);
    lab.y = cos(angle) * chroma;
    lab.z = sin(angle) * chroma;
    lab = defringe(lab, hue);
    // Black & white renders have the mix's grid.
    if offset(P_GRAY_GRID) >= 0 {
        // `black_white::gray`: the chart tables scale the color to its gray.
        let scaled = mixer(max(lab_to_srgb(lab), vec3(0.0)), offset(P_GRAY_GRID));
        let y = max(dot(scaled, vec3(0.2126, 0.7152, 0.0722)), 0.0);
        lab.x = clamp(pow(y, 1.0 / 3.0), 0.0, 1.0);
        lab.y = 0.0;
        lab.z = 0.0;
    }
    return lab;
}
// S1: scene_color.
fn scene_color(sample: vec3<f32>, apply_local: bool) -> vec3<f32> {
    var wb = vec3(1.0);
    if (V_MASKS && masked && apply_local) {
        let temp = vec3(p(P_LOCAL_WB), p(P_LOCAL_WB + 1u), p(P_LOCAL_WB + 2u));
        let tint = vec3(p(P_LOCAL_WB + 3u), p(P_LOCAL_WB + 4u), p(P_LOCAL_WB + 5u));
        wb = exp2(delta[L_TEMPERATURE] * temp + delta[L_TINT] * tint);
    }
    // Each channel held at the camera stage's clip; not `min`, which a NaN sample
    // would take to it.
    let clip = vec3(p(P_CAMERA_CLIP), p(P_CAMERA_CLIP + 1u), p(P_CAMERA_CLIP + 2u));
    let held = select(sample, clip, sample > clip);
    let c = held * vec3(p(P_WB), p(P_WB + 1u), p(P_WB + 2u)) * wb;
    var pro = matrix(P_CAMERA) * c;
    let hue = offset(P_HUE);
    if hue >= 0 {
        // An HDR profile's tables look up overrange-encoded values (`camera_color`).
        if p(P_HDR_PROFILE) != 0.0 {
            pro = table_apply_overrange(table_at(P_HUE), pro, offset(P_HUE2), p(P_HUE_WEIGHT));
        } else {
            pro = table_apply(table_at(P_HUE), pro, offset(P_HUE2), p(P_HUE_WEIGHT));
        }
    }
    return calibrate(PRO_TO_RGB * pro * p(P_PROFILE_SCALE));
}
// S2: exposure_stage, returning linear ProPhoto.
fn exposure_stage(color: vec3<f32>, apply_local: bool) -> vec3<f32> {
    var wide = TO_2020 * color;
    if (V_MASKS && masked && apply_local) {
        let exposure = exp2(delta[L_EXPOSURE]);
        let tint = exp2(vec3(delta[L_COLOR], delta[L_COLOR + 1u], delta[L_COLOR + 2u]));
        wide = wide * p(P_EXPOSURE) * exposure * tint;
    } else {
        wide = wide * p(P_EXPOSURE);
    }
    return RGB_TO_PRO * (FROM_2020 * wide);
}
// S3: scene_stage.
fn scene_stage(pro_in: vec3<f32>, pos: vec2<f32>, clarity_lum: f32) -> vec3<f32> {
    var pro = pro_in;
    var tone_gain = 1.0;
    if V_LOCAL && p(P_LOCAL) != 0.0 {
        tone_gain = local_gain(pos, dot(pro, vec3(0.2880402, 0.7118741, 0.0000857)), clarity_lum);
        pro *= tone_gain;
    }
    var dehaze = p(P_DEHAZE);
    if (V_MASKS && masked) {
        dehaze += delta[L_DEHAZE];
    }
    if !V_DEHAZE {
        dehaze = 0.0;
    }
    if dehaze > 0.0 {
        pro = dehaze_positive(pro, dehaze, pos, tone_gain);
    } else if dehaze < 0.0 {
        let key = p(P_DEHAZE + 1u);
        for (var c = 0u; c < 3u; c++) {
            if pro[c] > 0.0 {
                pro[c] *= exp2(family(2u, dehaze, key, log2(max(pro[c], LOCAL_FLOOR)), log2(max(pro[c], LOCAL_FLOOR))));
            }
        }
    }
    let toned = scene_global(pro);
    if (V_MASKS && masked) && (delta[L_WHITES] != 0.0 || delta[L_BLACKS] != 0.0) {
        return scene_mask(toned, delta[L_WHITES], delta[L_BLACKS]);
    }
    return toned;
}
fn process_pixel(i: u32, pos: vec2<f32>) -> vec3<f32> {
    point_selection = -1.0;
    var color: vec3<f32>;
    if size.z == 1u {
        color = vec3(kept[i * 3u], kept[i * 3u + 1u], kept[i * 3u + 2u]);
    } else {
        color = scene_color(vec3(samples[i * 3u], samples[i * 3u + 1u], samples[i * 3u + 2u]), true);
        if size.z == 2u {
            kept[i * 3u] = color.x;
            kept[i * 3u + 1u] = color.y;
            kept[i * 3u + 2u] = color.z;
        }
    }
    let scene_in = exposure_stage(color, true);
    var clarity_scene = scene_in;
    // Clarity reads fine detail on the same pre-mask basis as its measured blur.
    if (V_LOCAL && V_MASKS && masked) && offset(P_LOCAL_A + 2u) >= 0 {
        if delta[L_TEMPERATURE] != 0.0 || delta[L_TINT] != 0.0 || delta[L_EXPOSURE] != 0.0
            || delta[L_COLOR] != 0.0 || delta[L_COLOR + 1u] != 0.0 || delta[L_COLOR + 2u] != 0.0 {
            var original = color;
            if delta[L_TEMPERATURE] != 0.0 || delta[L_TINT] != 0.0 {
                original = scene_color(vec3(samples[i * 3u], samples[i * 3u + 1u], samples[i * 3u + 2u]), false);
            }
            clarity_scene = exposure_stage(original, false);
        }
    }
    let clarity_lum = dot(clarity_scene, vec3(0.2880402, 0.7118741, 0.0000857));
    let scene = scene_stage(scene_in, pos, clarity_lum);
    // An HDR master is written as it is after the profile, unclipped.
    if V_HDR {
        return hdr_finish(scene);
    }
    // S4: profile_stage.
    var rgb = profile_finish(scene);
    // color_stage
    rgb = reference_curves(rgb);
    if V_COLOR && offset(P_MIXER) >= 0 {
        // Saturation below −50: a fade to the luminance the color has
        // through the other sliders.
        var source = rgb;
        if offset(P_GRAY_SOURCE) >= 0 {
            source = mixer(rgb, offset(P_GRAY_SOURCE));
        }
        let y = dot(source, vec3(0.2126, 0.7152, 0.0722));
        rgb = mixer(rgb, offset(P_MIXER));
        rgb = mix(rgb, vec3(y), p(P_SATURATION_GRAY));
    }
    if V_COLOR && offset(P_POINT) >= 0 {
        rgb = point_colors(rgb);
    }
    if V_COLOR && offset(P_RGB) >= 0 && p(P_RGB + 5u) != 0.0 {
        rgb = rgb_table(rgb);
    }
    if V_COLOR && offset(P_GRADE) >= 0 {
        rgb = grade(rgb);
    }
    var lab = srgb_to_lab(rgb);
    if (V_MASKS && masked) && (delta[L_HUE] != 0.0 || delta[L_SATURATION] != 0.0) {
        let angle = radians(delta[L_HUE]);
        let k = max(1.0 + delta[L_SATURATION], 0.0);
        let a = lab.y;
        let b = lab.z;
        lab.y = (a * cos(angle) - b * sin(angle)) * k;
        lab.z = (a * sin(angle) + b * cos(angle)) * k;
    }
    if V_COLOR && p(P_ADJUST) != 0.0 {
        lab = adjust(lab);
    } else {
        lab.x = clamp(lab.x, 0.0, 1.0);
    }
    rgb = lab_to_srgb(lab);
    // Each channel clipped on its own, as Camera Raw does.
    var out: vec3<f32>;
    for (var k = 0; k < 3; k++) {
        out[k] = clamp(srgb_encode(clamp(rgb[k], 0.0, 1.0)), 0.0, 1.0);
    }
    if V_COLOR && point_selection >= 0.0 {
        out = visualize(out);
    }
    return out;
}

@compute @workgroup_size(256)
fn develop(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let i = (group.y * size.y + group.x) * 256u + local;
    if i >= size.x {
        return;
    }
    let pos = vec2(positions[i * 2u], positions[i * 2u + 1u]);
    var out = vec3(1.0);
    // Positions outside the photo are uploaded as OUTSIDE; Lightroom shows white there.
    if pos.x > OUTSIDE {
        load_delta(i);
        out = process_pixel(i, pos);
    }
    output[i * 3u] = out.x;
    output[i * 3u + 1u] = out.y;
    output[i * 3u + 2u] = out.z;
}
