// Full-resolution stages on the photo kept on the device: the image reduced for the
// Shadows/Highlights map (`pipeline::preview_source`) and region sampling through
// geometry, lens correction and noise reduction (`pipeline::sample_region`). The CPU
// versions are the reference; functions keep their names and order. Every entry point
// uses its own bindings.

// Sampling parameters, at the `S_*` offsets that `sampling::wgsl_prelude` puts
// before this file; radial tables follow the header (`sampling::HEADER`).
@group(0) @binding(10) var<storage, read> photo: array<f32>;
@group(0) @binding(12) var<storage, read> sp: array<f32>;
@group(0) @binding(13) var<storage, read_write> reduced: array<f32>;
@group(0) @binding(14) var<storage, read_write> samples: array<f32>;
@group(0) @binding(15) var<storage, read_write> positions: array<f32>;

const OUTSIDE: f32 = -3e38;

fn s(i: u32) -> f32 {
    return sp[i];
}
fn su(i: u32) -> u32 {
    return u32(sp[i]);
}
// The photo's pixel `i` (`pipeline::Source::px`).
fn px(i: u32) -> vec3<f32> {
    return vec3(photo[3u * i], photo[3u * i + 1u], photo[3u * i + 2u]);
}
fn table_eval(field: u32, r: f32) -> f32 {
    let at = u32(s(field));
    let len = su(field + 1u);
    // `Radial::eval`'s `partition_point(|x| *x <= r)`: the knots at or below `r`. The
    // window `i..i + n` shrinks by half each round, so seven reach any table of the 64
    // knots `optics::Radial` allows.
    var i = 0u;
    var n = len;
    for (var k = 0u; k < 7u && n > 0u; k++) {
        let half = n / 2u;
        if sp[at + i + half] <= r {
            i += half + 1u;
            n -= half + 1u;
        } else {
            n = half;
        }
    }
    if i == 0u {
        return sp[at + len];
    }
    if i == len {
        return sp[at + 2u * len - 1u];
    }
    let t = (r - sp[at + i - 1u]) / (sp[at + i] - sp[at + i - 1u]);
    let a = sp[at + len + i - 1u];
    return a + (sp[at + len + i] - a) * t;
}
fn has(field: u32) -> bool {
    return s(field) >= 0.0;
}
// `pipeline::sample`: bilinear, clamped to the photo.
fn sample(x_in: f32, y_in: f32) -> vec3<f32> {
    let w = su(S_WIDTH);
    let h = su(S_HEIGHT);
    let x = clamp(x_in, 0.0, f32(w - 1u));
    let y = clamp(y_in, 0.0, f32(h - 1u));
    let ix = u32(x);
    let iy = u32(y);
    let fx = x - f32(ix);
    let fy = y - f32(iy);
    let x1 = min(ix + 1u, w - 1u);
    let y1 = min(iy + 1u, h - 1u);
    let a = px(iy * w + ix);
    let b = px(iy * w + x1);
    let c = px(y1 * w + ix);
    let d = px(y1 * w + x1);
    return (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy;
}
fn detail_sample(x: f32, y: f32) -> vec3<f32> {
    let p = sample(x, y);
    let luma = s(S_NOISE);
    let chroma = s(S_NOISE + 1u);
    if luma == 0.0 && chroma == 0.0 {
        return p;
    }
    let center = (p.x + 2.0 * p.y + p.z) / 4.0;
    var sum = vec3(0.0);
    var total = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = sample(x + f32(dx), y + f32(dy));
            let lum = (q.x + 2.0 * q.y + q.z) / 4.0;
            let detail = (s(S_NOISE + 2u) + s(S_NOISE + 3u)) * 0.5;
            let threshold = 0.0025 * exp2((0.5 - detail) * 4.0);
            let d = lum - center;
            let w = 1.0 / (1.0 + d * d / threshold);
            sum += q * w;
            total += w;
        }
    }
    let avg = sum / total;
    let avgl = (avg.x + 2.0 * avg.y + avg.z) / 4.0;
    return center
        + (avgl - center) * luma * (1.0 - s(S_NOISE + 4u) * 0.5)
        + (p - center) * (1.0 - chroma)
        + (avg - avgl) * chroma * (0.5 + s(S_NOISE + 5u));
}
fn footprint_sample(x: f32, y: f32) -> vec3<f32> {
    let spread = s(S_SPREAD);
    if spread == 0.0 {
        return detail_sample(x, y);
    }
    var sum = vec3(0.0);
    for (var k = 0u; k < 4u; k++) {
        let dx = select(-1.0, 1.0, (k & 1u) == 1u);
        let dy = select(-1.0, 1.0, k >= 2u);
        sum += detail_sample(x + dx * spread, y + dy * spread) * 0.25;
    }
    return sum;
}
// `pipeline::LensWarp::sample`.
fn warp_sample(x: f32, y: f32) -> vec3<f32> {
    let cx = s(S_CENTER);
    let cy = s(S_CENTER + 1u);
    let dx = (x + 0.5 - cx) * s(S_FILL);
    let dy = (y + 0.5 - cy) * s(S_FILL);
    let r = sqrt(dx * dx + dy * dy) / s(S_HALF);
    var g = 1.0;
    if has(S_DISTORTION) {
        g = 1.0 + (table_eval(S_DISTORTION, r) - 1.0) * s(S_AMOUNT);
    }
    var scale = vec3(g);
    if has(S_RED) {
        // `lens_gpu_params`: a measured aberration (S_LENS 2) is evaluated at the
        // distorted radius.
        let rc = select(r, r * g, s(S_LENS) > 1.5);
        scale = vec3(g * table_eval(S_RED, rc), g, g * table_eval(S_BLUE, rc));
    }
    let gx = cx + dx * scale.y - 0.5;
    let gy = cy + dy * scale.y - 0.5;
    var p: vec3<f32>;
    if scale.x == scale.y && scale.z == scale.y {
        p = footprint_sample(gx, gy);
    } else {
        p = vec3(
            footprint_sample(cx + dx * scale.x - 0.5, cy + dy * scale.x - 0.5).x,
            footprint_sample(gx, gy).y,
            footprint_sample(cx + dx * scale.z - 0.5, cy + dy * scale.z - 0.5).z,
        );
    }
    var gain = 1.0;
    if has(S_VIGNETTING) {
        let vx = gx + 0.5 - cx;
        let vy = gy + 0.5 - cy;
        gain = powf(table_eval(S_VIGNETTING, sqrt(vx * vx + vy * vy) / s(S_HALF)), s(S_VIGNETTING_AMOUNT));
    }
    return p * gain;
}
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return select(0.0, 1.0, y == 0.0);
    }
    return pow(x, y);
}
// `Geometry::source`: output position (u, v in 0..1) to photo pixel coordinates.
fn source(u: f32, v: f32) -> vec2<f32> {
    let ow = s(S_ORIENTED);
    let oh = s(S_ORIENTED + 1u);
    var x = s(S_CROP) + u * (s(S_CROP + 2u) - s(S_CROP));
    var y = s(S_CROP + 1u) + v * (s(S_CROP + 3u) - s(S_CROP + 1u));
    x = (x - 0.5) * ow / s(S_ZOOM);
    y = (y - 0.5) * oh / s(S_ZOOM);
    let sn = s(S_SIN);
    let cs = s(S_COS);
    var nx = (cs * x + sn * y) / ow + 0.5;
    var ny = (-sn * x + cs * y) / oh + 0.5;
    if s(S_FLIP) != 0.0 { nx = 1.0 - nx; }
    if s(S_FLIP + 1u) != 0.0 { ny = 1.0 - ny; }
    var ox = nx;
    var oy = ny;
    switch su(S_TURNS) {
        case 1u: { ox = ny; oy = 1.0 - nx; }
        case 2u: { ox = 1.0 - nx; oy = 1.0 - ny; }
        case 3u: { ox = 1.0 - ny; oy = nx; }
        default: {}
    }
    if s(S_TRANSFORM) != 0.0 {
        let hm = S_HOMOGRAPHY;
        var w = s(hm + 6u) * ox + s(hm + 7u) * oy + s(hm + 8u);
        if !(w > 1e-6) { w = 1e-6; }
        let tx = (s(hm) * ox + s(hm + 1u) * oy + s(hm + 2u)) / w;
        oy = (s(hm + 3u) * ox + s(hm + 4u) * oy + s(hm + 5u)) / w;
        ox = tx;
    }
    // `ManualDistortion::source`.
    let k = s(S_MANUAL);
    if k != 0.0 {
        let dx = (ox - 0.5) * 2.0 * s(S_MANUAL + 1u);
        let dy = (oy - 0.5) * 2.0 * s(S_MANUAL + 2u);
        let rho = sqrt(dx * dx + dy * dy);
        var g = 1.0 + k;
        if rho > 1e-6 {
            var r = rho;
            var extra = 0.0;
            if k > 0.0 {
                let turn = sqrt((1.0 + k) / (3.0 * k));
                if rho > turn {
                    r = turn;
                    extra = rho - turn;
                }
            }
            g = (r * (1.0 + k * (1.0 - r * r)) + extra) / rho;
        }
        ox = 0.5 + (ox - 0.5) * g;
        oy = 0.5 + (oy - 0.5) * g;
    }
    return vec2(
        (s(S_INSET) + ox * s(S_INSET + 2u)) * s(S_WIDTH) - 0.5,
        (s(S_INSET + 1u) + oy * s(S_INSET + 3u)) * s(S_HEIGHT) - 0.5,
    );
}
@compute @workgroup_size(256)
fn sample_region(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let i = (group.y * su(S_COUNT) + group.x) * 256u + local;
    let rw = su(S_REGION + 2u);
    if i >= rw * su(S_REGION + 3u) { return; }
    let x = su(S_REGION) + i % rw;
    let y = su(S_REGION + 1u) + i / rw;
    let at = source((f32(x) + 0.5) / s(S_OUT), (f32(y) + 0.5) / s(S_OUT + 1u));
    // Beyond the camera's default crop counts as outside, as `Geometry::outside`.
    let x0 = s(S_INSET) * s(S_WIDTH) - 0.5;
    let y0 = s(S_INSET + 1u) * s(S_HEIGHT) - 0.5;
    let x1 = (s(S_INSET) + s(S_INSET + 2u)) * s(S_WIDTH) - 0.5;
    let y1 = (s(S_INSET + 1u) + s(S_INSET + 3u)) * s(S_HEIGHT) - 0.5;
    let outside = (s(S_TRANSFORM) != 0.0 || s(S_MANUAL) != 0.0) && (at.x < x0 || at.y < y0 || at.x > x1 || at.y > y1);
    var p = vec3(1.0);
    var pos = vec2(OUTSIDE);
    if !outside {
        if s(S_LENS) != 0.0 {
            p = warp_sample(at.x, at.y);
        } else {
            p = footprint_sample(at.x, at.y);
        }
        pos = at;
    }
    samples[3u * i] = p.x;
    samples[3u * i + 1u] = p.y;
    samples[3u * i + 2u] = p.z;
    positions[2u * i] = pos.x;
    positions[2u * i + 1u] = pos.y;
}
// `pipeline::preview_source`: box integration of the toned photo, in two passes so
// neighbouring invocations read neighbouring pixels: each row's sums over the boxes'
// columns, then each box's sum over its rows.
@group(0) @binding(9) var<storage, read_write> partial: array<f32>;

fn box_columns(x: u32) -> vec2<u32> {
    let iw = su(S_WIDTH);
    let w = su(S_REDUCED);
    let x0 = x * iw / w;
    return vec2(x0, max((x + 1u) * iw / w, x0 + 1u));
}
@compute @workgroup_size(16, 16)
fn reduce_rows(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = su(S_REDUCED);
    if id.x >= w || id.y >= su(S_HEIGHT) { return; }
    let span = box_columns(id.x);
    let row = id.y * su(S_WIDTH);
    var sum = vec3(0.0);
    for (var x = span.x; x < span.y; x++) {
        sum += px(row + x);
    }
    let i = 3u * (id.y * w + id.x);
    partial[i] = sum.x;
    partial[i + 1u] = sum.y;
    partial[i + 2u] = sum.z;
}
@compute @workgroup_size(16, 16)
fn reduce_toned(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = su(S_REDUCED);
    let h = su(S_REDUCED + 1u);
    if id.x >= w || id.y >= h { return; }
    let ih = su(S_HEIGHT);
    let span = box_columns(id.x);
    let y0 = id.y * ih / h;
    let y1 = max((id.y + 1u) * ih / h, y0 + 1u);
    var out = vec3(0.0);
    for (var y = y0; y < y1; y++) {
        let i = 3u * (y * w + id.x);
        out += vec3(partial[i], partial[i + 1u], partial[i + 2u]);
    }
    let n = f32((span.y - span.x) * (y1 - y0));
    let i = id.y * w + id.x;
    reduced[3u * i] = out.x / n;
    reduced[3u * i + 1u] = out.y / n;
    reduced[3u * i + 2u] = out.z / n;
}
