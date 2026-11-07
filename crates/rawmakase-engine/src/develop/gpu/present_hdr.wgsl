// An HDR master's preview finishing after the develop stage's HDR variant, on its
// pixels still on the device (linear ProPhoto, 1 = graphics white): sharpening on
// their sRGB-curve-encoded values, unclipped, with ProPhoto luminance; the HDR limit;
// then extended sRGB for the window, each channel held to the display's headroom,
// into a half-float texture. Ports of `quality::hdr::master` after `render_base`
// and of `color::hdr_output::extended_srgb`; the CPU versions are the reference.
struct Params {
    // Developed buffer size, and the rectangle of it that is shown.
    width: u32, height: u32, crop_x: u32, crop_y: u32,
    crop_w: u32, crop_h: u32, radius: u32, sharpen: u32,
    amount: f32, threshold: f32, halo: f32, dark: f32,
    // The HDR limit's peak and the display's headroom, both over graphics white.
    peak: f32, headroom: f32, pad0: u32, pad1: u32,
    // Linear ProPhoto to linear sRGB primaries, row by row.
    m00: f32, m01: f32, m02: f32, m10: f32,
    m11: f32, m12: f32, m20: f32, m21: f32,
    m22: f32, pad2: u32, pad3: u32, pad4: u32,
};
@group(0) @binding(0) var<storage, read_write> pixels: array<f32>;
@group(0) @binding(1) var<storage, read_write> scratch: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<f32>;
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(4) var shown: texture_storage_2d<rgba16float, write>;

fn rgb(i: u32) -> vec3<f32> {
    return vec3(pixels[3u * i], pixels[3u * i + 1u], pixels[3u * i + 2u]);
}
// `OutputSpace::ProPhoto`'s luminance, which the master sharpens with.
fn luminance(v: vec3<f32>) -> f32 {
    return 0.28807113 * v.r + 0.7118432 * v.g + 0.000085654 * v.b;
}
fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}
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
// Sharpening works on the sRGB-curve-encoded values; the present pass decodes them.
@compute @workgroup_size(16, 16)
fn encode(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    let i = 3u * (id.y * p.width + id.x);
    for (var c = 0u; c < 3u; c++) {
        pixels[i + c] = srgb_encode(max(pixels[i + c], 0.0));
    }
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
// `sharpening::Sharpener::delta`, as present.wgsl has it.
fn sharpen_delta(d: f32, amount: f32, threshold: f32, halo: f32, dark: f32) -> f32 {
    var mask = 1.0;
    if threshold != 0.0 { mask = clamp(abs(d) / threshold, 0.0, 1.0); }
    let k = amount * mask;
    let r = abs(d) / halo;
    let shaped = d / (1.0 + r * r * r);
    return k * select(shaped, shaped * dark, shaped < 0.0);
}
// `quality::sharpen_unbounded`: the detail added unclipped above white.
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
    let result = max(color + vec3(delta), vec3(0.0));
    pixels[3u * i] = result.r;
    pixels[3u * i + 1u] = result.g;
    pixels[3u * i + 2u] = result.b;
}
// The master's limit, then `extended_srgb`: linear sRGB primaries, negative where
// outside sRGB, each channel's magnitude held to the headroom, the sRGB curve
// mirrored below 0.
@compute @workgroup_size(16, 16)
fn present(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.crop_w || id.y >= p.crop_h { return; }
    var c = rgb((p.crop_y + id.y) * p.width + p.crop_x + id.x);
    if p.sharpen != 0u {
        c = vec3(srgb_decode(max(c.r, 0.0)), srgb_decode(max(c.g, 0.0)), srgb_decode(max(c.b, 0.0)));
    }
    c = vec3(roll_off(c.r, p.peak), roll_off(c.g, p.peak), roll_off(c.b, p.peak));
    let s = vec3(
        p.m00 * c.r + p.m01 * c.g + p.m02 * c.b,
        p.m10 * c.r + p.m11 * c.g + p.m12 * c.b,
        p.m20 * c.r + p.m21 * c.g + p.m22 * c.b,
    );
    var out = vec3(0.0);
    for (var k = 0; k < 3; k++) {
        let v = select(s[k], 0.0, !(s[k] == s[k]));
        out[k] = sign(v) * srgb_encode(roll_off(abs(v), p.headroom));
    }
    textureStore(shown, vec2(id.x, id.y), vec4(out, 1.0));
}
