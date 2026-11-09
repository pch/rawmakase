// Full-resolution stages on the photo kept on the device: region sampling through
// geometry, lens correction and noise reduction (`pipeline::sample_region`). The CPU
// versions are the reference; functions keep their names and order. Every entry point
// uses its own bindings.

// Sampling parameters, at the `S_*` offsets that `sampling::wgsl_prelude` puts
// before this file; radial tables follow the header (`sampling::HEADER`).
@group(0) @binding(10) var<storage, read> photo: array<f32>;
// The transparency mask (`Metadata::coverage`), four values to a word.
@group(0) @binding(11) var<storage, read> coverage: array<u32>;
@group(0) @binding(12) var<storage, read> sp: array<f32>;
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
    // window `i..i + n` shrinks by half each round, so `RADIAL_STEPS` reach any table of
    // the knots `optics::Radial` allows (`sampling::radial_steps`).
    var i = 0u;
    var n = len;
    for (var k = 0u; k < RADIAL_STEPS && n > 0u; k++) {
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
fn footprint_sample(x: f32, y: f32) -> vec3<f32> {
    let spread = s(S_SPREAD);
    if spread == 0.0 {
        return sample(x, y);
    }
    var sum = vec3(0.0);
    for (var k = 0u; k < 4u; k++) {
        let dx = select(-1.0, 1.0, (k & 1u) == 1u);
        let dy = select(-1.0, 1.0, k >= 2u);
        sum += sample(x + dx * spread, y + dy * spread) * 0.25;
    }
    return sum;
}
// Corrected colour and the decoded position used by local tone maps.
struct LensSample {
    color: vec3<f32>,
    position: vec2<f32>,
}
// `pipeline::LensWarp::sample` and `LensMap::forward`.
fn warp_sample(x: f32, y: f32) -> LensSample {
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
        let red = footprint_sample(cx + dx * scale.x - 0.5, cy + dy * scale.x - 0.5);
        let green = footprint_sample(gx, gy);
        let blue = footprint_sample(cx + dx * scale.z - 0.5, cy + dy * scale.z - 0.5);
        p = vec3(red.x, green.y, blue.z);
        if s(S_XTRANS) > 0.5 {
            p = stripe_ca_sample(gx, gy, red, green, blue);
        }
    }
    var gain = 1.0;
    if has(S_VIGNETTING) {
        let vx = gx + 0.5 - cx;
        let vy = gy + 0.5 - cy;
        gain = powf(table_eval(S_VIGNETTING, sqrt(vx * vx + vy * vy) / s(S_HALF)), s(S_VIGNETTING_AMOUNT));
    }
    if has(S_GAIN_MAP) {
        // After the opcode list's warp: the corrected position.
        if s(su(S_GAIN_MAP) + 13u) > 0.5 {
            return LensSample(p * gain * gain_map(cx + dx - 0.5, cy + dy - 0.5), vec2(gx, gy));
        }
        // Before it: each channel where it samples the photo.
        let red = gain_map(cx + dx * scale.x - 0.5, cy + dy * scale.x - 0.5).x;
        let blue = gain_map(cx + dx * scale.z - 0.5, cy + dy * scale.z - 0.5).z;
        return LensSample(p * gain * vec3(red, gain_map(gx, gy).y, blue), vec2(gx, gy));
    }
    return LensSample(p * gain, vec2(gx, gy));
}
// `GainField::gain`: a DNG's OpcodeList3 GainMap, per channel, at the table that
// `lens_gpu_params` appends.
fn gain_map(x: f32, y: f32) -> vec3<f32> {
    let at = su(S_GAIN_MAP);
    let rows = su(at);
    let columns = su(at + 1u);
    let v = (y + 0.5) / s(at + 7u);
    let h = (x + 0.5) / s(at + 6u);
    if v < s(at + 9u) || v >= s(at + 11u) || h < s(at + 10u) || h >= s(at + 12u) {
        return vec3(1.0);
    }
    let fv = clamp((v - s(at + 2u)) / s(at + 4u), 0.0, f32(rows - 1u));
    let fh = clamp((h - s(at + 3u)) / s(at + 5u), 0.0, f32(columns - 1u));
    let r0 = min(u32(fv), select(rows - 2u, 0u, rows < 2u));
    let c0 = min(u32(fh), select(columns - 2u, 0u, columns < 2u));
    let r1 = min(r0 + 1u, rows - 1u);
    let c1 = min(c0 + 1u, columns - 1u);
    let tv = fv - f32(r0);
    let th = fh - f32(c0);
    let g = at + 14u;
    var out = vec3(1.0);
    for (var c = 0u; c < 3u; c++) {
        let a = sp[g + (r0 * columns + c0) * 3u + c];
        let b = sp[g + (r0 * columns + c1) * 3u + c];
        let d = sp[g + (r1 * columns + c0) * 3u + c];
        let e = sp[g + (r1 * columns + c1) * 3u + c];
        let top = a + (b - a) * th;
        let bottom = d + (e - d) * th;
        out[c] = powf(max(top + (bottom - top) * tv, 0.0), s(at + 8u));
    }
    return out;
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
// `CameraImage::empty_at`: whether photo position `pos` lies where the file has
// no image, the nearest pixel of its transparency mask less than half covered.
fn empty(pos: vec2<f32>) -> bool {
    let cw = su(S_COVERAGE);
    if cw == 0u {
        return false;
    }
    let ch = su(S_COVERAGE + 1u);
    let cx = u32(clamp(floor((pos.x + 0.5) / s(S_WIDTH) * f32(cw)), 0.0, f32(cw - 1u)));
    let cy = u32(clamp(floor((pos.y + 0.5) / s(S_HEIGHT) * f32(ch)), 0.0, f32(ch - 1u)));
    let i = cy * cw + cx;
    return ((coverage[i / 4u] >> (8u * (i % 4u))) & 255u) < 128u;
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
            let corrected = warp_sample(at.x, at.y);
            p = corrected.color;
            pos = corrected.position;
        } else {
            p = footprint_sample(at.x, at.y);
            pos = at;
        }
        // Where the file has no image (a panorama's empty corners), white, as
        // outside the photo.
        if empty(pos) {
            p = vec3(1.0);
            pos = vec2(OUTSIDE);
        }
    }
    samples[3u * i] = p.x;
    samples[3u * i + 1u] = p.y;
    samples[3u * i + 2u] = p.z;
    positions[2u * i] = pos.x;
    positions[2u * i + 1u] = pos.y;
}
fn stripe_luma(p: vec3<f32>) -> f32 {
    return dot(p, vec3(0.25, 0.5, 0.25));
}
fn stripe_chroma(p: vec3<f32>) -> vec2<f32> {
    return vec2(p.x - p.y, p.z - p.y) / max(stripe_luma(p), 1e-8);
}
fn stripe_periodic(x: f32, y: f32, dx: i32, dy: i32) -> vec2<f32> {
    var values: array<f32, 7>;
    var low = vec2(1e30);
    var high = vec2(-1e30);
    var lo = 1e30;
    var hi = -1e30;
    for (var i = 0; i < 7; i++) {
        let k = i - 3;
        let xx = clamp(i32(round(x)) + k * dx, 0, i32(su(S_WIDTH)) - 1);
        let yy = clamp(i32(round(y)) + k * dy, 0, i32(su(S_HEIGHT)) - 1);
        let p = px(u32(yy) * su(S_WIDTH) + u32(xx));
        let colour = stripe_chroma(p);
        low = min(low, colour);
        high = max(high, colour);
        let l = sqrt(max(stripe_luma(p), 0.0));
        values[i] = l;
        lo = min(lo, l);
        hi = max(hi, l);
    }
    var previous = 0.0;
    var turns = 0u;
    for (var i = 0; i < 6; i++) {
        let d = values[i + 1] - values[i];
        if abs(d) > (hi - lo) * 0.05 {
            if d * previous < 0.0 { turns += 1u; }
            previous = d;
        }
    }
    let range = high - low;
    let colour = clamp((0.0025 - dot(range, range)) / 0.0021, 0.0, 1.0);
    if colour == 0.0 { return vec2(0.0); }
    if turns < 2u { return vec2(0.0, select(0.0, 1.0, colour == 1.0)); }
    var coherence = 0.0;
    // Symmetric, then either one-sided five-line neighbourhood, matching CPU.
    for (var window = 0; window < 3; window++) {
        if coherence >= 0.7 { break; }
        if window > 0 {
            let reach = select(12, -12, window == 2);
            let end_x = i32(round(x)) + reach * dy;
            let end_y = i32(round(y)) + reach * dx;
            if end_x < 0 || end_x >= i32(su(S_WIDTH)) || end_y < 0 || end_y >= i32(su(S_HEIGHT)) {
                continue;
            }
        }
        var average = values;
        var total = stripe_variance(values);
        let offsets = array<i32, 4>(-6, -3, 3, 6);
        for (var side = 0; side < 4; side++) {
            var across = offsets[side];
            if window > 0 {
                across = (side + 1) * 3;
                if window == 2 { across = -across; }
            }
            var line: array<f32, 7>;
            for (var i = 0; i < 7; i++) {
                let k = i - 3;
                let xx = clamp(i32(round(x)) + k * dx + across * dy, 0, i32(su(S_WIDTH)) - 1);
                let yy = clamp(i32(round(y)) + k * dy + across * dx, 0, i32(su(S_HEIGHT)) - 1);
                line[i] = sqrt(max(stripe_luma(px(u32(yy) * su(S_WIDTH) + u32(xx))), 0.0));
                average[i] += line[i];
            }
            total += stripe_variance(line);
        }
        for (var i = 0; i < 7; i++) { average[i] /= 5.0; }
        coherence = max(coherence, stripe_variance(average) / max(total / 5.0, 1e-8));
    }
    return vec2(colour * clamp((coherence - 0.55) / 0.15, 0.0, 1.0), 0.0);
}
fn stripe_variance(line: array<f32, 7>) -> f32 {
    var mean = 0.0;
    for (var i = 0; i < 7; i++) { mean += line[i]; }
    mean /= 7.0;
    var variance = 0.0;
    for (var i = 0; i < 7; i++) { let d = line[i] - mean; variance += d * d; }
    return variance / 7.0;
}
// `pipeline::fields::stripe_ca_sample`.
fn stripe_ca_sample(x: f32, y: f32, red: vec3<f32>, green: vec3<f32>, blue: vec3<f32>) -> vec3<f32> {
    let p = vec3(red.x, green.y, blue.z);
    let base = stripe_chroma(green);
    let d = stripe_chroma(p) - base;
    let dr = stripe_chroma(red) - base;
    let db = stripe_chroma(blue) - base;
    let introduced = dot(d, d);
    let variation = max(dot(dr, dr), dot(db, db));
    if introduced < 1e-8 || variation >= 0.16 * introduced { return p; }
    var stripe_weight = 0.0;
    for (var axis = 0; axis < 2; axis++) {
        let dx = 1 - axis;
        let dy = axis;
        let local = stripe_periodic(x, y, dx, dy);
        var evidence = local.x;
        if local.y > 0.5 && variation < 1e-6 {
            for (var side = -3; side <= 3; side += 6) {
                let nx = x + f32(side * dy);
                let ny = y + f32(side * dx);
                let xx = i32(round(nx));
                let yy = i32(round(ny));
                if xx < 0 || xx >= i32(su(S_WIDTH)) || yy < 0 || yy >= i32(su(S_HEIGHT)) { continue; }
                let difference = stripe_chroma(px(u32(yy) * su(S_WIDTH) + u32(xx))) - base;
                let matching = clamp(1.0 - dot(difference, difference) / 0.000004, 0.0, 1.0);
                if matching > 0.0 {
                    evidence = max(evidence, matching * stripe_periodic(nx, ny, dx, dy).x);
                    if evidence == 1.0 { break; }
                }
            }
        }
        stripe_weight = max(stripe_weight, evidence);
        if stripe_weight == 1.0 { break; }
    }
    let keep = stripe_weight * clamp(1.0 - (sqrt(variation / introduced) - 0.1) / 0.3, 0.0, 1.0);
    let gain = stripe_luma(p) / max(stripe_luma(green), 1e-8);
    return p + keep * (green * gain - p);
}
