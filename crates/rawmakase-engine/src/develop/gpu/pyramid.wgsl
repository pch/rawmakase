// Shared GPU Laplacian pyramid compute kernels for Color Noise Reduction
// (`color_noise.rs`), Luminance Noise Reduction (`luminance_noise.rs`), Texture
// (`texture.rs`), and crop-clamped box reduction (`pipeline/sampling.rs`).
//
// Every entry point uses a single 4-binding layout (`src_a` read, `src_b` read,
// `dst` read_write, `u` uniform) with no buffer aliasing and 16x16 workgroups
// over `(u.dst_w, u.dst_h)`.

struct Params {
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    aux0: u32,
    aux1: u32,
    aux2: u32,
    aux3: u32,
    f0: f32,
    f1: f32,
    f2: f32,
    f3: f32,
}

@group(0) @binding(0) var<storage, read> src_a: array<f32>;
@group(0) @binding(1) var<storage, read> src_b: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;
@group(0) @binding(3) var<uniform> u: Params;

fn mirror(v_in: i32, n_u: u32) -> u32 {
    let n = i32(n_u);
    let v = abs(v_in);
    let r = select(v, 2 * n - 2 - v, v >= n);
    return u32(clamp(r, 0, n - 1));
}

fn partial_gain(gain: f32, share: f32) -> f32 {
    return select(1.0 - share * (1.0 - gain), gain, share >= 1.0);
}

// Match `color_noise::clip_at_black` so GPU NR, Texture and measurement box
// reductions stay identical to CPU on signed below-black demosaiced pixels.
fn clip_at_black(p: vec3<f32>) -> vec3<f32> {
    let lowest = min(p.x, min(p.y, p.z));
    if (lowest >= 0.0) {
        return p;
    }
    let pos = max(p, vec3<f32>(0.0));
    let target_luma = (pos.x + 2.0 * pos.y + pos.z) * 0.25;
    let own = (p.x + 2.0 * p.y + p.z) * 0.25;
    if (own <= 0.0) {
        return vec3<f32>(target_luma);
    }
    let keep = own / (own - lowest);
    let scale = target_luma / own;
    return max((vec3<f32>(own) + (p - vec3<f32>(own)) * keep) * scale, vec3<f32>(0.0));
}

fn sqrt_luma(r: f32, g: f32, b: f32) -> f32 {
    let c = clip_at_black(vec3<f32>(r, g, b));
    return (sqrt(max(c.x, 0.0)) + 2.0 * sqrt(max(c.y, 0.0)) + sqrt(max(c.z, 0.0))) * 0.25;
}

fn read_vec2(buf_idx: u32) -> vec2<f32> {
    let base = buf_idx * 2u;
    return vec2<f32>(src_a[base], src_a[base + 1u]);
}

fn up_vec2_vertical(x: u32, y: u32) -> vec2<f32> {
    let c = i32(y / 2u);
    if ((y & 1u) == 0u) {
        let r0 = mirror(c - 1, u.src_h) * u.dst_w + x;
        let r1 = mirror(c, u.src_h) * u.dst_w + x;
        let r2 = mirror(c + 1, u.src_h) * u.dst_w + x;
        return read_vec2(r0) * 0.125 + read_vec2(r1) * 0.75 + read_vec2(r2) * 0.125;
    } else {
        let r0 = mirror(c, u.src_h) * u.dst_w + x;
        let r1 = mirror(c + 1, u.src_h) * u.dst_w + x;
        return (read_vec2(r0) + read_vec2(r1)) * 0.5;
    }
}

fn up_f32_vertical(x: u32, y: u32) -> f32 {
    let c = i32(y / 2u);
    if ((y & 1u) == 0u) {
        let r0 = mirror(c - 1, u.src_h) * u.dst_w + x;
        let r1 = mirror(c, u.src_h) * u.dst_w + x;
        let r2 = mirror(c + 1, u.src_h) * u.dst_w + x;
        return src_a[r0] * 0.125 + src_a[r1] * 0.75 + src_a[r2] * 0.125;
    } else {
        let r0 = mirror(c, u.src_h) * u.dst_w + x;
        let r1 = mirror(c + 1, u.src_h) * u.dst_w + x;
        return (src_a[r0] + src_a[r1]) * 0.5;
    }
}

fn up_f32_vertical_b(x: u32, y: u32) -> f32 {
    let c = i32(y / 2u);
    if ((y & 1u) == 0u) {
        let r0 = mirror(c - 1, u.src_h) * u.dst_w + x;
        let r1 = mirror(c, u.src_h) * u.dst_w + x;
        let r2 = mirror(c + 1, u.src_h) * u.dst_w + x;
        return src_b[r0] * 0.125 + src_b[r1] * 0.75 + src_b[r2] * 0.125;
    } else {
        let r0 = mirror(c, u.src_h) * u.dst_w + x;
        let r1 = mirror(c + 1, u.src_h) * u.dst_w + x;
        return (src_b[r0] + src_b[r1]) * 0.5;
    }
}

// ============================================================================
// Color Noise Reduction (2-channel square-root chroma pyramid)
// ============================================================================

@compute @workgroup_size(16, 16)
fn chroma_extract(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let p = i * 3u;
    let c = clip_at_black(vec3<f32>(src_a[p], src_a[p + 1u], src_a[p + 2u]));
    let r = sqrt(max(c.x, 0.0));
    let g = sqrt(max(c.y, 0.0));
    let b = sqrt(max(c.z, 0.0));
    let out = i * 2u;
    dst[out] = r - g;
    dst[out + 1u] = b - g;
}

@compute @workgroup_size(16, 16)
fn down_vec2_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let row = y * u.src_w;
    let ix = 2 * i32(x);
    let v = read_vec2(row + mirror(ix - 2, u.src_w)) * 0.0625
          + read_vec2(row + mirror(ix - 1, u.src_w)) * 0.25
          + read_vec2(row + mirror(ix, u.src_w)) * 0.375
          + read_vec2(row + mirror(ix + 1, u.src_w)) * 0.25
          + read_vec2(row + mirror(ix + 2, u.src_w)) * 0.0625;
    let out = (y * u.dst_w + x) * 2u;
    dst[out] = v.x;
    dst[out + 1u] = v.y;
}

@compute @workgroup_size(16, 16)
fn down_vec2_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let iy = 2 * i32(y);
    let v = read_vec2(mirror(iy - 2, u.src_h) * u.dst_w + x) * 0.0625
          + read_vec2(mirror(iy - 1, u.src_h) * u.dst_w + x) * 0.25
          + read_vec2(mirror(iy, u.src_h) * u.dst_w + x) * 0.375
          + read_vec2(mirror(iy + 1, u.src_h) * u.dst_w + x) * 0.25
          + read_vec2(mirror(iy + 2, u.src_h) * u.dst_w + x) * 0.0625;
    let out = (y * u.dst_w + x) * 2u;
    dst[out] = v.x;
    dst[out + 1u] = v.y;
}

@compute @workgroup_size(16, 16)
fn up_vec2_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let row = y * u.src_w;
    let c = i32(x / 2u);
    var v: vec2<f32>;
    if ((x & 1u) == 0u) {
        v = read_vec2(row + mirror(c - 1, u.src_w)) * 0.125
          + read_vec2(row + mirror(c, u.src_w)) * 0.75
          + read_vec2(row + mirror(c + 1, u.src_w)) * 0.125;
    } else {
        v = (read_vec2(row + mirror(c, u.src_w)) + read_vec2(row + mirror(c + 1, u.src_w))) * 0.5;
    }
    let out = (y * u.dst_w + x) * 2u;
    dst[out] = v.x;
    dst[out + 1u] = v.y;
}

@compute @workgroup_size(16, 16)
fn up_vec2_v_sub(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let up = up_vec2_vertical(x, y);
    let out = (y * u.dst_w + x) * 2u;
    dst[out] = dst[out] - up.x;
    dst[out + 1u] = dst[out + 1u] - up.y;
}

// Level-0 chroma 8x8 block stats reading detail from `src_a` and camera RGB from `src_b`.
// `u.f0..f2` = wb, `u.f3` = MEASURABLE_PEAK, `u.aux0` = output float offset.
@compute @workgroup_size(16, 16)
fn chroma_block_stats_l0(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bx = gid.x;
    let by = gid.y;
    if (bx >= u.dst_w || by >= u.dst_h) { return; }
    let out_idx = u.aux0 + (by * u.dst_w + bx) * 2u;
    let x0 = bx * 8u;
    let y0 = by * 8u;
    var sum = 0.0;
    var black = 0.0;
    for (var dy = 0u; dy < 8u; dy++) {
        let row = (y0 + dy) * u.src_w + x0;
        for (var dx = 0u; dx < 8u; dx++) {
            let i = row + dx;
            let p = i * 3u;
            let r = src_b[p];
            let g = src_b[p + 1u];
            let b = src_b[p + 2u];
            if (!(r / u.f0 < u.f3 && g / u.f1 < u.f3 && b / u.f2 < u.f3)) {
                dst[out_idx] = -1.0;
                dst[out_idx + 1u] = 0.0;
                return;
            }
            let c = clip_at_black(vec3<f32>(r, g, b));
            if (c.x <= 0.0 || c.y <= 0.0 || c.z <= 0.0) {
                black += 1.0;
            }
            let d = read_vec2(i);
            sum += d.x * d.x + d.y * d.y;
        }
    }
    dst[out_idx] = sum / 64.0;
    dst[out_idx + 1u] = black / 64.0;
}

// Coarser-level chroma 8x8 block stats reading detail from `src_a` and packed mask from `src_b`.
// Mask entry `< 0.0` marks clipped; `>= 0.0` holds the black share.
@compute @workgroup_size(16, 16)
fn chroma_block_stats(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bx = gid.x;
    let by = gid.y;
    if (bx >= u.dst_w || by >= u.dst_h) { return; }
    let out_idx = u.aux0 + (by * u.dst_w + bx) * 2u;
    let x0 = bx * 8u;
    let y0 = by * 8u;
    var sum = 0.0;
    var black = 0.0;
    for (var dy = 0u; dy < 8u; dy++) {
        let row = (y0 + dy) * u.src_w + x0;
        for (var dx = 0u; dx < 8u; dx++) {
            let i = row + dx;
            let m = src_b[i];
            if (m < 0.0) {
                dst[out_idx] = -1.0;
                dst[out_idx + 1u] = 0.0;
                return;
            }
            black += m;
            let d = read_vec2(i);
            sum += d.x * d.x + d.y * d.y;
        }
    }
    dst[out_idx] = sum / 64.0;
    dst[out_idx + 1u] = black / 64.0;
}

// Downsample measurability mask directly from camera RGB (`src_a`) to level 1 (`dst`).
@compute @workgroup_size(16, 16)
fn mask_down_l0(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    var unclipped = true;
    var black = 0.0;
    let ix = 2 * i32(x);
    let iy = 2 * i32(y);
    for (var dy = -2; dy <= 2; dy++) {
        let row = mirror(iy + dy, u.src_h) * u.src_w;
        for (var dx = -2; dx <= 2; dx++) {
            let p = (row + mirror(ix + dx, u.src_w)) * 3u;
            let r = src_a[p];
            let g = src_a[p + 1u];
            let b = src_a[p + 2u];
            if (!(r / u.f0 < u.f3 && g / u.f1 < u.f3 && b / u.f2 < u.f3)) {
                unclipped = false;
            }
            let c = clip_at_black(vec3<f32>(r, g, b));
            if (c.x <= 0.0 || c.y <= 0.0 || c.z <= 0.0) {
                black += 1.0 / 25.0;
            }
        }
    }
    dst[y * u.dst_w + x] = select(-1.0, black, unclipped);
}

// Downsample packed measurability mask (`src_a`) by 2x into `dst`.
@compute @workgroup_size(16, 16)
fn mask_down(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    var unclipped = true;
    var black = 0.0;
    let ix = 2 * i32(x);
    let iy = 2 * i32(y);
    for (var dy = -2; dy <= 2; dy++) {
        let row = mirror(iy + dy, u.src_h) * u.src_w;
        for (var dx = -2; dx <= 2; dx++) {
            let m = src_a[row + mirror(ix + dx, u.src_w)];
            if (m < 0.0) {
                unclipped = false;
            } else {
                black += m / 25.0;
            }
        }
    }
    dst[y * u.dst_w + x] = select(-1.0, black, unclipped);
}

// Reconstruct chroma level in-place: `dst` holds detail, `src_a` holds horizontally expanded coarse.
// `u.f0` = threshold, `u.f1` = filtered share.
@compute @workgroup_size(16, 16)
fn up_vec2_v_reconstruct_chroma(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let c = up_vec2_vertical(x, y);
    let out = (y * u.dst_w + x) * 2u;
    let d = vec2<f32>(dst[out], dst[out + 1u]);
    let size = d.x * d.x + d.y * d.y;
    var gain = 1.0;
    if (u.f0 > 0.0) {
        if (size <= 0.0) {
            gain = 0.0;
        } else {
            let ratio = (u.f0 * u.f0) / size;
            if (ratio > 1e12) {
                gain = 0.0;
            } else {
                let p3 = ratio * ratio * ratio;
                gain = 1.0 / (1.0 + p3);
            }
        }
    }
    let g = partial_gain(gain, u.f1);
    dst[out] = c.x + d.x * g;
    dst[out + 1u] = c.y + d.y * g;
}

// Write denoised chroma (`src_a`) back into camera RGB (`dst`), preserving square-root luminance.
@compute @workgroup_size(16, 16)
fn chroma_writeback(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let p = i * 3u;
    let luma = sqrt_luma(dst[p], dst[p + 1u], dst[p + 2u]);
    let ch = read_vec2(i);
    let g = luma - (ch.x + ch.y) * 0.25;
    let sr = max(g + ch.x, 0.0);
    let sg = max(g, 0.0);
    let sb = max(g + ch.y, 0.0);
    dst[p] = sr * sr;
    dst[p + 1u] = sg * sg;
    dst[p + 2u] = sb * sb;
}

// ============================================================================
// Single-channel Laplacian primitives (shared by Luminance NR & Texture)
// ============================================================================

@compute @workgroup_size(16, 16)
fn luma_extract(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let p = i * 3u;
    dst[i] = sqrt_luma(src_a[p], src_a[p + 1u], src_a[p + 2u]);
}

@compute @workgroup_size(16, 16)
fn down_f32_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let row = y * u.src_w;
    let ix = 2 * i32(x);
    dst[y * u.dst_w + x] =
        src_a[row + mirror(ix - 2, u.src_w)] * 0.0625
      + src_a[row + mirror(ix - 1, u.src_w)] * 0.25
      + src_a[row + mirror(ix, u.src_w)] * 0.375
      + src_a[row + mirror(ix + 1, u.src_w)] * 0.25
      + src_a[row + mirror(ix + 2, u.src_w)] * 0.0625;
}

@compute @workgroup_size(16, 16)
fn down_f32_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let iy = 2 * i32(y);
    dst[y * u.dst_w + x] =
        src_a[mirror(iy - 2, u.src_h) * u.dst_w + x] * 0.0625
      + src_a[mirror(iy - 1, u.src_h) * u.dst_w + x] * 0.25
      + src_a[mirror(iy, u.src_h) * u.dst_w + x] * 0.375
      + src_a[mirror(iy + 1, u.src_h) * u.dst_w + x] * 0.25
      + src_a[mirror(iy + 2, u.src_h) * u.dst_w + x] * 0.0625;
}

@compute @workgroup_size(16, 16)
fn up_f32_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let row = y * u.src_w;
    let c = i32(x / 2u);
    var v: f32;
    if ((x & 1u) == 0u) {
        v = src_a[row + mirror(c - 1, u.src_w)] * 0.125
          + src_a[row + mirror(c, u.src_w)] * 0.75
          + src_a[row + mirror(c + 1, u.src_w)] * 0.125;
    } else {
        v = (src_a[row + mirror(c, u.src_w)] + src_a[row + mirror(c + 1, u.src_w)]) * 0.5;
    }
    dst[y * u.dst_w + x] = v;
}

// Compute `dst = src_a - up_vertical(src_b)` without modifying `src_a` (needed for
// luminance noise block stats, which read both the Gaussian level and its detail band).
@compute @workgroup_size(16, 16)
fn up_f32_v_sub(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    dst[i] = src_a[i] - up_f32_vertical_b(x, y);
}

// ============================================================================
// Luminance Noise Reduction
// ============================================================================

// Luminance 8x8 block stats: `src_a` = Gaussian plane, `src_b` = detail band, `u.f0` = white.
@compute @workgroup_size(16, 16)
fn luma_block_stats(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bx = gid.x;
    let by = gid.y;
    if (bx >= u.dst_w || by >= u.dst_h) { return; }
    let out_idx = u.aux0 + by * u.dst_w + bx;
    let x0 = bx * 8u;
    let y0 = by * 8u;
    var energy = 0.0;
    var valid = 0u;
    for (var dy = 0u; dy < 8u; dy++) {
        let row = (y0 + dy) * u.src_w + x0;
        for (var dx = 0u; dx < 8u; dx++) {
            let i = row + dx;
            let v = src_a[i];
            if (v >= u.f0) {
                dst[out_idx] = -1.0;
                return;
            }
            if (v > 1e-5) {
                valid += 1u;
            }
            let b = src_b[i];
            energy += b * b;
        }
    }
    dst[out_idx] = select(-1.0, energy / 64.0, valid >= 8u);
}

// Reconstruct luminance level: `src_a` = horizontally expanded coarse, `src_b` = detail band,
// `dst` = reconstructed level. `u.f0` = threshold, `u.f1` = filtered share.
@compute @workgroup_size(16, 16)
fn up_f32_v_reconstruct_luma(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let out = up_f32_vertical(x, y);
    var energy = 0.0;
    let ix = i32(x);
    let iy = i32(y);
    for (var dy = -1; dy <= 1; dy++) {
        let row = mirror(iy + dy, u.dst_h) * u.dst_w;
        for (var dx = -1; dx <= 1; dx++) {
            let v = src_b[row + mirror(ix + dx, u.dst_w)];
            energy += (v * v) / 9.0;
        }
    }
    var gain = 1.0;
    if (u.f0 > 0.0) {
        let ratio = (u.f0 * u.f0) / max(energy, 1e-20);
        if (ratio > 1e18) {
            gain = 0.0;
        } else {
            gain = 1.0 / (1.0 + ratio * ratio);
        }
    }
    let i = y * u.dst_w + x;
    dst[i] = out + src_b[i] * partial_gain(gain, u.f1);
}

// Write denoised luminance (`src_a`) back into camera RGB (`dst`), preserving square-root chroma.
@compute @workgroup_size(16, 16)
fn luma_writeback(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let p = i * 3u;
    let c = clip_at_black(vec3<f32>(dst[p], dst[p + 1u], dst[p + 2u]));
    let orig = (sqrt(max(c.x, 0.0)) + 2.0 * sqrt(max(c.y, 0.0)) + sqrt(max(c.z, 0.0))) * 0.25;
    let delta = src_a[i] - orig;
    if (delta != 0.0) {
        let sr = max(sqrt(max(c.x, 0.0)) + delta, 0.0);
        let sg = max(sqrt(max(c.y, 0.0)) + delta, 0.0);
        let sb = max(sqrt(max(c.z, 0.0)) + delta, 0.0);
        dst[p] = sr * sr;
        dst[p + 1u] = sg * sg;
        dst[p + 2u] = sb * sb;
    }
}

// ============================================================================
// Texture (per-channel log2 Laplacian pyramid)
// ============================================================================

// Portable `log2(x)` for positive finite `x` (~3 ULP worst-case, ~4e-8 absolute error
// on `[0.5, 2)` vs WGSL's allowed `2^-21` ≈ `4.8e-7`), reducing vendor-dependent
// rounding jitter at Texture's 1/8192 EV quantization boundary.
fn exact_log2(x: f32) -> f32 {
    let bits = bitcast<u32>(x);
    var e = i32((bits >> 23u) & 0xffu) - 127;
    var m = bitcast<f32>((bits & 0x007fffffu) | 0x3f800000u);
    if (m >= 1.41421356) {
        m *= 0.5;
        e += 1;
    }
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let poly = t * (2.8853900818 + t2 * (0.9617966939 + t2 * (0.5770780164 + t2 * (0.4121985831 + t2 * 0.3205988980))));
    return f32(e) + poly;
}

// Extract `log2(max(clip_at_black(photo)[c], 1e-6))` for channel `u.aux0` into `dst`.
@compute @workgroup_size(16, 16)
fn texture_extract(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let p = i * 3u;
    let c = clip_at_black(vec3<f32>(src_a[p], src_a[p + 1u], src_a[p + 2u]));
    let ch = select(select(c.x, c.y, u.aux0 == 1u), c.z, u.aux0 == 2u);
    dst[i] = exact_log2(max(ch, 1e-6));
}

// Overwrite `dst` (Gaussian level) in-place with compressed weighted detail:
// `band = dst - up_vertical(src_a)`, `dst = u.f0 * band / (1 + (band / u.f1)^2)`.
@compute @workgroup_size(16, 16)
fn up_f32_v_compress_texture(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let band = dst[i] - up_f32_vertical(x, y);
    let r = band / u.f1;
    dst[i] = u.f0 * band / (1.0 + r * r);
}

// Accumulate expanded coarser texture detail (`src_a`) in-place into `dst`.
@compute @workgroup_size(16, 16)
fn up_f32_v_add_inplace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    dst[i] = dst[i] + up_f32_vertical(x, y);
}

// Apply Texture detail (`src_a`) to channel `u.aux0` of camera RGB (`dst`), quantized to
// `u.f1` (`DETAIL_STEP`) and clamped to ±32767 steps to match `TextureDetail::apply`.
// `u.f0` = strength(amount), `u.f1` = DETAIL_STEP.
@compute @workgroup_size(16, 16)
fn texture_apply_channel(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let i = y * u.dst_w + x;
    let q = clamp(round(src_a[i] / u.f1), -32767.0, 32767.0);
    let idx = i * 3u + u.aux0;
    if (dst[idx] > 0.0) {
        dst[idx] = dst[idx] * exp2(u.f0 * u.f1 * q);
    }
}

// ============================================================================
// Crop-clamped box reduction (measurement copy for Shadows/Highlights & scene tone)
// ============================================================================

// Box-reduce `src_a` (`src_w` x `src_h` RGB) into `dst` (`dst_w` x `dst_h` RGB), clamping
// source coordinates to `[aux0..aux2)` x `[aux1..aux3)` (`crop_clamped_preview`).
@compute @workgroup_size(16, 16)
fn box_reduce(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if (x >= u.dst_w || y >= u.dst_h) { return; }
    let x0 = x * u.src_w / u.dst_w;
    let x1 = max((x + 1u) * u.src_w / u.dst_w, x0 + 1u);
    let y0 = y * u.src_h / u.dst_h;
    let y1 = max((y + 1u) * u.src_h / u.dst_h, y0 + 1u);
    let cx_min = u.aux0;
    let cy_min = u.aux1;
    let cx_max = u.aux2 - 1u;
    let cy_max = u.aux3 - 1u;
    var acc = vec3<f32>(0.0);
    for (var yy = y0; yy < y1; yy++) {
        let cy = clamp(yy, cy_min, cy_max);
        let row = cy * u.src_w;
        for (var xx = x0; xx < x1; xx++) {
            let cx = clamp(xx, cx_min, cx_max);
            let p = (row + cx) * 3u;
            acc += clip_at_black(vec3<f32>(src_a[p], src_a[p + 1u], src_a[p + 2u]));
        }
    }
    let n = f32((x1 - x0) * (y1 - y0));
    let out = (y * u.dst_w + x) * 3u;
    dst[out] = acc.x / n;
    dst[out + 1u] = acc.y / n;
    dst[out + 2u] = acc.z / n;
}
