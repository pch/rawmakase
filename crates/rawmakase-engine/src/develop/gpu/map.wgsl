// The engine 4 Shadows/Highlights map's base on the device (`local_tone::MapBase`):
// the guided filter of log2 luminance of the reduced photo toned, and the image keys
// (luminance percentiles) found by radix select. Each box mean sums its window
// directly rather than with running sums, so no error accumulates along a line.
struct Map {
    width: u32,
    height: u32,
    radius: u32,
    // `cols`: 0 makes the guided filter's coefficients of the means, 1 keeps the means.
    mode: u32,
    epsilon: f32,
}
@group(0) @binding(0) var<uniform> m: Map;
@group(0) @binding(1) var<storage, read> src: array<f32>;
// Two planes of width × height.
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;
// Luminance as bits, which order as the (positive) values do.
@group(0) @binding(3) var<storage, read_write> bits: array<u32>;
// Per key, 256 bins of the digit being selected.
@group(0) @binding(4) var<storage, read_write> hist: array<atomic<u32>, 512>;
// Per key the selected digits so far, then per key the rank left to find among the
// values with those digits, then the round (0 to 3, most significant digit first).
@group(0) @binding(5) var<storage, read_write> state: array<u32, 5>;
// log2 of the two selected luminances: the Shadows and Highlights keys.
@group(0) @binding(6) var<storage, read_write> keys: array<f32, 2>;

fn pixel(group: vec3<u32>, local: u32) -> u32 {
    return (group.y * 65535u + group.x) * 256u + local;
}

// The toned pixels' log2 luminance and its square, and the luminance's bits.
@compute @workgroup_size(256)
fn prepare(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let n = m.width * m.height;
    let i = pixel(group, local);
    if i >= n {
        return;
    }
    let y = max(0.2126 * src[i * 3u] + 0.7152 * src[i * 3u + 1u] + 0.0722 * src[i * 3u + 2u], 6e-4);
    let l = log2(y);
    dst[i] = l;
    dst[n + i] = l * l;
    bits[i] = bitcast<u32>(y);
}

// The mean along the row over 2r + 1 pixels, clamped at the borders, of both planes.
@compute @workgroup_size(256)
fn rows(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let n = m.width * m.height;
    let i = pixel(group, local);
    if i >= n {
        return;
    }
    let x = i32(i % m.width);
    let line = i - u32(x);
    let r = i32(m.radius);
    var sum = vec2(0.0);
    for (var j = -r; j <= r; j++) {
        let k = line + u32(clamp(x + j, 0, i32(m.width) - 1));
        sum += vec2(src[k], src[n + k]);
    }
    let mean = sum / f32(2 * r + 1);
    dst[i] = mean.x;
    dst[n + i] = mean.y;
}

// The mean along the column, then (mode 0) the guided filter's coefficients a and b of
// the means of the values and their squares.
@compute @workgroup_size(256)
fn cols(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let n = m.width * m.height;
    let i = pixel(group, local);
    if i >= n {
        return;
    }
    let x = i % m.width;
    let y = i32(i / m.width);
    let r = i32(m.radius);
    var sum = vec2(0.0);
    for (var j = -r; j <= r; j++) {
        let k = u32(clamp(y + j, 0, i32(m.height) - 1)) * m.width + x;
        sum += vec2(src[k], src[n + k]);
    }
    var out = sum / f32(2 * r + 1);
    if m.mode == 0u {
        let v = max(out.y - out.x * out.x, 0.0);
        let a = v / (v + m.epsilon);
        out = vec2(a, out.x - a * out.x);
    }
    dst[i] = out.x;
    dst[n + i] = out.y;
}

var<workgroup> counts: array<atomic<u32>, 512>;

// Counts the current digit of the values that have the digits selected so far, per key,
// in workgroup memory, then adds each workgroup's counts once.
@compute @workgroup_size(256)
fn histogram(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    atomicStore(&counts[local], 0u);
    atomicStore(&counts[local + 256u], 0u);
    workgroupBarrier();
    let i = pixel(group, local);
    if i < m.width * m.height {
        let round = state[4];
        let shift = 24u - 8u * round;
        // The digits above this one; none in the first round.
        let above = select(0u, ~((1u << (shift + 8u)) - 1u), round > 0u);
        let b = bits[i];
        let digit = (b >> shift) & 255u;
        for (var t = 0u; t < 2u; t++) {
            if (b & above) == state[t] {
                atomicAdd(&counts[t * 256u + digit], 1u);
            }
        }
    }
    workgroupBarrier();
    for (var j = local; j < 512u; j += 256u) {
        let c = atomicLoad(&counts[j]);
        if c > 0u {
            atomicAdd(&hist[j], c);
        }
    }
}

// Per key, the digit holding the rank left, and after the last round the key.
@compute @workgroup_size(2)
fn resolve(@builtin(local_invocation_index) t: u32) {
    let round = state[4];
    let shift = 24u - 8u * round;
    var k = state[2u + t];
    var below = 0u;
    for (var d = 0u; d < 256u; d++) {
        let c = atomicLoad(&hist[t * 256u + d]);
        if k < below + c {
            state[t] |= d << shift;
            state[2u + t] = k - below;
            break;
        }
        below += c;
    }
    if round == 3u {
        keys[t] = log2(bitcast<f32>(state[t]));
    }
    storageBarrier();
    if t == 0u {
        state[4] = round + 1u;
    }
}
