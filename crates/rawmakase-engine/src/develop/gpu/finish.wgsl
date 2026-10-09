// Packed RGB buffers avoid padding each pixel to 16 bytes. Pass boundaries provide
// synchronization; sharpening modifies only its own pixel after the horizontal pass.
struct Params {
    width: u32, height: u32, out_width: u32, out_height: u32,
    radius: u32, x_stride: u32, y_stride: u32, y_offset: u32,
    amount: f32, threshold: f32, halo: f32, dark: f32,
};
@group(0) @binding(0) var<storage, read_write> source: array<f32>;
@group(0) @binding(1) var<storage, read_write> scratch: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
@group(0) @binding(3) var<storage, read> weights: array<f32>;
@group(0) @binding(4) var<uniform> p: Params;

fn rgb(i: u32) -> vec3<f32> {
    return vec3(source[3u*i], source[3u*i+1u], source[3u*i+2u]);
}
fn luminance(v: vec3<f32>) -> f32 {
    return 0.2126*v.r + 0.7152*v.g + 0.0722*v.b;
}
@compute @workgroup_size(16, 16)
fn blur_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    var value = 0.0;
    for (var k = 0u; k <= 2u*p.radius; k++) {
        let x = u32(clamp(i32(id.x)+i32(k)-i32(p.radius), 0, i32(p.width)-1));
        value += luminance(rgb(id.y*p.width+x))*weights[k];
    }
    scratch[id.y*p.width+id.x] = value;
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
    for (var k = 0u; k <= 2u*p.radius; k++) {
        let y = u32(clamp(i32(id.y)+i32(k)-i32(p.radius), 0, i32(p.height)-1));
        blur += scratch[y*p.width+id.x]*weights[k];
    }
    let i = id.y*p.width+id.x;
    let color = rgb(i);
    let d = luminance(color)-blur;
    let delta = sharpen_delta(d, p.amount, p.threshold, p.halo, p.dark);
    let result = clamp(color+vec3(delta), vec3(0.0), vec3(1.0));
    source[3u*i] = result.r;
    source[3u*i+1u] = result.g;
    source[3u*i+2u] = result.b;
}
@compute @workgroup_size(16, 16)
fn resize_vertical(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.out_height { return; }
    let offset = p.y_offset+id.y*p.y_stride;
    let start = u32(weights[offset]);
    let count = u32(weights[offset+1u]);
    var sum = vec3(0.0);
    for (var k = 0u; k < count; k++) {
        sum += rgb((start+k)*p.width+id.x)*weights[offset+2u+k];
    }
    let i = 3u*(id.y*p.width+id.x);
    scratch[i] = sum.r;
    scratch[i+1u] = sum.g;
    scratch[i+2u] = sum.b;
}
@compute @workgroup_size(16, 16)
fn resize_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.out_width || id.y >= p.out_height { return; }
    let offset = 2u*p.radius+1u+id.x*p.x_stride;
    let start = u32(weights[offset]);
    let count = u32(weights[offset+1u]);
    var sum = vec3(0.0);
    for (var k = 0u; k < count; k++) {
        let i = 3u*(id.y*p.width+start+k);
        sum += vec3(scratch[i],scratch[i+1u],scratch[i+2u])*weights[offset+2u+k];
    }
    let result = clamp(sum, vec3(0.0), vec3(1.0));
    let i = 3u*(id.y*p.out_width+id.x);
    output[i] = result.r;
    output[i+1u] = result.g;
    output[i+2u] = result.b;
}
