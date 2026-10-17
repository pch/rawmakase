//! Fujifilm's compressed RAF data (lossless and lossy), from bytes to
//! samples.
//!
//! A 16-byte header ("IS", 1 lossless or 0 lossy, the sensor: 0 Bayer, 16
//! X-Trans, the bits per sample, then big-endian the height, the width
//! rounded up to whole stripes, the width, the stripe width (768), the
//! number of stripes and the number of six-row groups) is followed by each
//! stripe's length (32-bit big-endian), padded to 16 bytes; lossy data then
//! has, per stripe, one quantiser per group, padded to 16 bytes; then the
//! stripes. Each stripe is a vertical band of 768 columns coded on its own,
//! most significant bit first, six rows at a time: the rows' samples are
//! gathered by colour into lines (per group three red, six green and three
//! blue; X-Trans lines hold slots the filter fills by interpolation, which
//! carry no bits), and the lines are coded in pairs (red and green, green and
//! blue, alternating), even positions leading odd ones by seven. Each sample
//! is predicted from its line's neighbours and the line above, and the
//! prediction error is Golomb-Rice coded, its parameter adapting per context
//! (the quantised gradients around the sample), with an escape for large
//! errors. Lossy groups quantise errors by a step of 2q + 1; smooth places
//! keep a finer step with their own statistics.
//!
//! Sources: the public prose description of Fujifilm's lossless RAF
//! compression on capnfabs.net (stripes, colour lines, iteration order,
//! neighbourhoods, Rice coding with gradient contexts), Fujifilm's
//! description of its compressed modes, and our own measurements, comparing
//! the decoded samples with LibRaw's recorded output on our test dataset.
use super::bits::Msb;
use crate::error::{Error, Result, corrupt};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Columns per stripe.
const STRIPE: usize = 768;

/// The X-Trans arrangement the coding assumes (0 red, 1 green, 2 blue).
const XTRANS: [[u8; 6]; 6] = [
    [1, 1, 0, 1, 1, 2],
    [1, 1, 2, 1, 1, 0],
    [2, 0, 1, 0, 2, 1],
    [1, 1, 2, 1, 1, 0],
    [1, 1, 0, 1, 1, 2],
    [0, 2, 1, 2, 0, 1],
];

/// Per pair of lines and per line of the pair, the X-Trans slots (bit `p %
/// 4` of position `p`) that are coded; the others are interpolated.
const CODED: [[u8; 2]; 6] = [[10, 15], [15, 10], [14, 10], [15, 11], [11, 15], [10, 14]];

/// The stream's header.
#[derive(Clone, Copy, Debug)]
struct Header {
    lossless: bool,
    xtrans: bool,
    bits: u32,
    stripes: usize,
    groups: usize,
}

fn header(src: &[u8], width: u32, height: u32, bits: u32, xtrans: bool) -> Result<Header> {
    let h = src
        .get(..16)
        .ok_or_else(|| corrupt("Fujifilm compressed header"))?;
    let be16 = |at: usize| u32::from(u16::from_be_bytes([h[at], h[at + 1]]));
    if &h[..2] != b"IS" || h[2] > 1 {
        return Err(corrupt("Fujifilm compressed header"));
    }
    if h[3] != if xtrans { 16 } else { 0 } {
        return Err(Error::Unsupported(
            "Fujifilm compressed data of another sensor layout".into(),
        ));
    }
    if !matches!(bits, 12 | 14 | 16) {
        return Err(Error::Unsupported(format!(
            "Fujifilm compressed data of {bits} bits"
        )));
    }
    let stripes = usize::from(h[13]);
    let groups = be16(14) as usize;
    if u32::from(h[4]) != bits
        || be16(5) != height
        || be16(9) != width
        || be16(11) != STRIPE as u32
        || stripes == 0
        || stripes != (width as usize).div_ceil(STRIPE)
        || be16(7) as usize != stripes * STRIPE
        || !height.is_multiple_of(6)
        || groups != height as usize / 6
    {
        return Err(corrupt(
            "Fujifilm compressed header disagrees with the image",
        ));
    }
    Ok(Header {
        lossless: h[2] == 1,
        xtrans,
        bits,
        stripes,
        groups,
    })
}

/// One stripe: its coded bytes, its quantisers (lossy) and its width.
struct Stripe<'a> {
    data: &'a [u8],
    quantisers: &'a [u8],
    width: usize,
}

/// Decodes `width × height` samples of `bits` from the compressed data
/// `src`, of a Bayer (RGGB) or X-Trans sensor.
pub(crate) fn decode(
    src: &[u8],
    width: u32,
    height: u32,
    bits: u32,
    xtrans: bool,
    cancel: &AtomicBool,
) -> Result<Vec<u16>> {
    let n = crate::io::samples(width, height, 1)?;
    let h = header(src, width, height, bits, xtrans)?;
    let w = width as usize;
    let table_end = (16 + 4 * h.stripes).next_multiple_of(16);
    let quantiser_len = h.groups.next_multiple_of(16);
    let mut at = table_end
        + if h.lossless {
            0
        } else {
            quantiser_len * h.stripes
        };
    let mut stripes = Vec::with_capacity(h.stripes);
    for i in 0..h.stripes {
        let e = src
            .get(16 + 4 * i..20 + 4 * i)
            .ok_or_else(|| corrupt("Fujifilm stripe lengths"))?;
        let len = u32::from_be_bytes([e[0], e[1], e[2], e[3]]) as usize;
        let data = at
            .checked_add(len)
            .and_then(|end| src.get(at..end))
            .ok_or_else(|| corrupt("Fujifilm stripe past the end of the data"))?;
        // Every sample of the stripe takes a bit at least: shorter data
        // cannot hold the image the header promises.
        if (len as u64) * 8 < (h.groups * 6 * STRIPE) as u64 {
            return Err(corrupt("Fujifilm stripe shorter than its samples"));
        }
        let quantisers = if h.lossless {
            &[][..]
        } else {
            let q = table_end + i * quantiser_len;
            src.get(q..q + h.groups)
                .ok_or_else(|| corrupt("Fujifilm quantisers"))?
        };
        stripes.push(Stripe {
            data,
            quantisers,
            width: (w - i * STRIPE).min(STRIPE),
        });
        at += len;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let decoded = stripes
        .par_iter()
        .map(|s| stripe(s, &h, cancel))
        .collect::<Result<Vec<_>>>()?;
    let mut out = crate::io::zeroed(n)?;
    for (i, (s, samples)) in stripes.iter().zip(decoded).enumerate() {
        for (row, src) in out.chunks_exact_mut(w).zip(samples.chunks_exact(s.width)) {
            row[i * STRIPE..i * STRIPE + s.width].copy_from_slice(src);
        }
    }
    Ok(out)
}

/// Adaptive Rice statistics of one context: the sum of the errors' sizes
/// and their count.
#[derive(Clone, Copy)]
struct Stats {
    sum: u32,
    count: u32,
}

impl Stats {
    fn new(range: u32) -> Self {
        Self {
            sum: ((range + 32) / 64).max(2),
            count: 1,
        }
    }
    /// The Rice parameter: the least k with count · 2^k ≥ sum.
    fn k(self) -> u32 {
        let mut k = 0;
        while k < 31 && self.count << k < self.sum {
            k += 1;
        }
        k
    }
    fn update(&mut self, error: i32) {
        self.sum = self.sum.saturating_add(error.unsigned_abs());
        if self.count == 64 {
            self.sum >>= 1;
            self.count >>= 1;
        }
        self.count += 1;
    }
}

/// How errors of a quantiser `q` are coded: the step 2q + 1, the number of
/// distinct errors and the bits of the escape.
#[derive(Clone, Copy)]
struct Quantiser {
    q: u32,
    range: u32,
    escape_bits: u32,
}

impl Quantiser {
    fn new(q: u32, max: u32) -> Self {
        let range = (max + 2 * q) / (2 * q + 1) + 1;
        Self {
            q,
            range,
            escape_bits: 32 - (range - 1).leading_zeros(),
        }
    }
}

/// The gradient `v` bucketed by the thresholds of quantiser `q`: 0 to 4,
/// signed.
fn bucket(v: i32, q: u32) -> i32 {
    let a = v.unsigned_abs();
    let b = if a <= q {
        0
    } else if a < 18 + 3 * q {
        1
    } else if a < 67 + 5 * q {
        2
    } else if a < 276 + 7 * q {
        3
    } else {
        4
    };
    if v < 0 { -b } else { b }
}

/// Reads one Rice-coded error: a run of zeros ended by a one, then `k`
/// bits; a run of `limit` zeros escapes to the error in `escape_bits`.
fn error(bits: &mut Msb, k: u32, quant: Quantiser, limit: u32) -> Result<i32> {
    let mut zeros = 0u32;
    loop {
        let next = bits.peek(32);
        if next != 0 {
            let z = next.leading_zeros();
            bits.skip(z + 1);
            zeros += z;
            break;
        }
        bits.skip(32);
        zeros += 32;
        if zeros > limit {
            break;
        }
    }
    if zeros > limit {
        return Err(corrupt("Fujifilm error code too long"));
    }
    let code = if zeros == limit {
        u64::from(bits.get(quant.escape_bits)) + 1
    } else {
        (u64::from(zeros) << k) | u64::from(bits.get(k))
    };
    if code > u64::from(quant.range) || bits.overrun() {
        return Err(corrupt("Fujifilm error code"));
    }
    let half = (code / 2) as i32;
    Ok(if code & 1 == 0 { half } else { -half - 1 })
}

/// The lines of one colour: the last two of the group before, then the
/// group's, each with a slot either side of its `size` samples.
struct Lines {
    rows: Vec<Vec<i32>>,
}

impl Lines {
    fn new(size: usize, count: usize) -> Self {
        Self {
            rows: vec![vec![0; size + 2]; 2 + count],
        }
    }
    /// Starts the group: the last two lines move to the top.
    fn next_group(&mut self) {
        let n = self.rows.len();
        self.rows.swap(0, n - 2);
        self.rows.swap(1, n - 1);
    }
}

/// Decodes one stripe: `height × width` samples.
fn stripe(s: &Stripe, h: &Header, cancel: &AtomicBool) -> Result<Vec<u16>> {
    let size = if h.xtrans { STRIPE * 2 / 3 } else { STRIPE / 2 };
    let max = (1u32 << h.bits) - 1;
    // Red, green and blue lines; per group red and blue have three, green six.
    let mut lines = [
        Lines::new(size, 3),
        Lines::new(size, 6),
        Lines::new(size, 3),
    ];
    let main = |q| [[Stats::new(Quantiser::new(q, max).range); 41]; 2];
    // Per pair of lines (modulo 3), per even and odd position.
    let mut stats = [main(0); 3];
    // Finer quantisers' statistics, kept across groups.
    let fine: [[Stats; 5]; 2] = [[Stats::new(Quantiser::new(0, max).range); 5]; 2];
    let mut finer = [
        [fine; 3],
        [[[Stats::new(Quantiser::new(1, max).range); 5]; 2]; 3],
        [[[Stats::new(Quantiser::new(2, max).range); 5]; 2]; 3],
    ];
    let limit_of = |q: Quantiser| 4 * h.bits - q.escape_bits - 1;
    let mut current = None;
    let mut bits = Msb::new(s.data);
    let mut out = Vec::new();
    out.try_reserve_exact(s.width * 6 * h.groups)
        .map_err(|_| Error::TooLarge("Fujifilm stripe"))?;
    for group in 0..h.groups {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let q = u32::from(s.quantisers.get(group).copied().unwrap_or(0));
        if current != Some(q) {
            stats = [main(q); 3];
            current = Some(q);
        }
        let quant = Quantiser::new(q, max);
        for l in &mut lines {
            l.next_group();
        }
        // How many lines of each colour the group has started.
        let mut started = [0usize; 3];
        for pair in 0..6 {
            let colours = if pair % 2 == 0 { [0, 1] } else { [1, 2] };
            let mut at = [0usize; 2];
            for (which, &c) in colours.iter().enumerate() {
                let y = 2 + started[c];
                started[c] += 1;
                at[which] = y;
                let rows = &mut lines[c].rows;
                let (first, last) = (rows[y - 1][1], rows[y - 1][size]);
                rows[y][0] = first;
                rows[y][size + 1] = last;
            }
            for even in (0..size + 8).step_by(2) {
                for (odd, pos) in [(0, Some(even)), (1, even.checked_sub(7))] {
                    let Some(pos) = pos.filter(|&p| p < size) else {
                        continue;
                    };
                    let x = pos + 1;
                    for which in 0..2 {
                        let c = colours[which];
                        let y = at[which];
                        let rows = &mut lines[c].rows;
                        let (nw, n, ne, nn) = (
                            rows[y - 1][x - 1],
                            rows[y - 1][x],
                            rows[y - 1][x + 1],
                            rows[y - 2][x],
                        );
                        let (west, east) = (rows[y][x - 1], rows[y][x + 1]);
                        let (predicted, g1, g2) = if odd == 0 {
                            let (a, b, c) = ((nw - n).abs(), (ne - n).abs(), (nn - n).abs());
                            let pair_sum = if a > b && a > c {
                                ne + nn
                            } else if b > a && b > c {
                                nw + nn
                            } else {
                                nw + ne
                            };
                            ((pair_sum + 2 * n) >> 2, n - nn, nw - n)
                        } else {
                            let between = n >= nw.min(ne) && n <= nw.max(ne);
                            let p = if between {
                                (west + east) >> 1
                            } else {
                                (west + east + 2 * n) >> 2
                            };
                            (p, n - nw, nw - west)
                        };
                        let coded = !h.xtrans || (CODED[pair][which] >> (pos % 4)) & 1 != 0;
                        let value = if coded {
                            let contrast = g1.unsigned_abs() + g2.unsigned_abs();
                            let precision = match contrast {
                                0..=5 => 0,
                                6 => q.min(1),
                                7 => q.min(2),
                                _ => q,
                            };
                            let (sign, st, used) = if precision < q {
                                let s = |v: i32| {
                                    if v.unsigned_abs() <= precision {
                                        0
                                    } else {
                                        v.signum()
                                    }
                                };
                                let ctx = 3 * s(g1) + s(g2);
                                let st = &mut finer[precision as usize][pair % 3][odd]
                                    [ctx.unsigned_abs() as usize];
                                (ctx, st, Quantiser::new(precision, max))
                            } else {
                                let ctx = 9 * bucket(g1, q) + bucket(g2, q);
                                let st = &mut stats[pair % 3][odd][ctx.unsigned_abs() as usize];
                                (ctx, st, quant)
                            };
                            let mut e = error(&mut bits, st.k(), used, limit_of(used))?;
                            st.update(e);
                            if sign < 0 {
                                e = -e;
                            }
                            let step = (2 * used.q + 1) as i32;
                            let mut v = predicted + e * step;
                            let range = used.range as i32 * step;
                            if v < -(used.q as i32) {
                                v += range;
                            } else if v > (max + used.q) as i32 {
                                v -= range;
                            }
                            v.clamp(0, max as i32)
                        } else {
                            predicted
                        };
                        rows[y][x] = value;
                    }
                }
            }
        }
        // The group's rows from the lines.
        for (y, xtrans_row) in XTRANS.iter().enumerate() {
            for x in 0..s.width {
                let c = if h.xtrans {
                    xtrans_row[x % 6]
                } else {
                    match (y % 2, x % 2) {
                        (0, 0) => 0,
                        (1, 1) => 2,
                        _ => 1,
                    }
                } as usize;
                let line = 2 + if c == 1 { y } else { y / 2 };
                let slot = 1 + if h.xtrans {
                    2 * (x / 3) + (x % 3).min(1)
                } else {
                    x / 2
                };
                out.push(lines[c].rows[line][slot] as u16);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bits, most significant first, into bytes.
    fn pack(bits: &[bool]) -> Vec<u8> {
        bits.chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, &v)| b | (u8::from(v) << (7 - i)))
            })
            .collect()
    }

    /// A stream whose every coded error is 0: one bit, then k zero bits, the
    /// statistics tracked as the decoder tracks them.
    pub(crate) fn zeros(bits: u32, xtrans: bool, groups: usize) -> Vec<u8> {
        let max = (1u32 << bits) - 1;
        let size: usize = if xtrans { 512 } else { 384 };
        let mut stats = [[[Stats::new(Quantiser::new(0, max).range); 41]; 2]; 3];
        let mut out = Vec::new();
        for _ in 0..groups {
            for pair in 0..6 {
                for even in (0..size + 8).step_by(2) {
                    for (odd, pos) in [(0, Some(even)), (1, even.checked_sub(7))] {
                        let Some(pos) = pos.filter(|&p| p < size) else {
                            continue;
                        };
                        for mask in CODED[pair] {
                            if xtrans && (mask >> (pos % 4)) & 1 == 0 {
                                continue;
                            }
                            // Flat lines: every gradient 0, context 0.
                            let st = &mut stats[pair % 3][odd][0];
                            out.push(true);
                            out.extend(std::iter::repeat_n(false, st.k() as usize));
                            st.update(0);
                        }
                    }
                }
            }
        }
        pack(&out)
    }

    /// Compressed data of two stripes of `width` × `6 groups` samples.
    pub(crate) fn stream(
        bits: u32,
        xtrans: bool,
        width: u16,
        groups: u16,
        stripe: &[u8],
        lossy: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut s = b"IS".to_vec();
        s.extend([
            u8::from(lossy.is_none()),
            if xtrans { 16 } else { 0 },
            bits as u8,
        ]);
        for v in [groups * 6, 2 * 768, width, 768] {
            s.extend(v.to_be_bytes());
        }
        s.push(2);
        s.extend(groups.to_be_bytes());
        for _ in 0..2 {
            s.extend((stripe.len() as u32).to_be_bytes());
        }
        s.resize(32, 0);
        if let Some(q) = lossy {
            for _ in 0..2 {
                let at = s.len();
                s.extend(q);
                s.resize(at + q.len().next_multiple_of(16), 0);
            }
        }
        s.extend(stripe);
        s.extend(stripe);
        s
    }

    #[test]
    fn errors_are_rice_coded_with_an_escape() {
        let q = Quantiser::new(0, 16383);
        assert_eq!((q.range, q.escape_bits), (16384, 14));
        // k = 2: 1 00 (0), 1 01 (−1), 1 10 (1), 1 11 (−2); then 41 zeros and
        // 14 bits of 16382: the escape for 16383, −8192.
        let mut b = vec![
            true, false, false, true, false, true, true, true, false, true, true, true,
        ];
        b.extend(std::iter::repeat_n(false, 41));
        b.push(true);
        b.extend((0..14).rev().map(|i| (16382u32 >> i) & 1 != 0));
        let data = pack(&b);
        let mut bits = Msb::new(&data);
        for want in [0, -1, 1, -2, -8192] {
            assert_eq!(error(&mut bits, 2, q, 41).unwrap(), want);
        }
        // The largest Rice parameter: no overflow, the code out of range.
        let mut bits = Msb::new(&[0, 0, 0, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert!(error(&mut bits, 31, q, 41).is_err());
        // A run longer than the escape is an error.
        let mut bits = Msb::new(&[0; 16]);
        assert!(error(&mut bits, 2, q, 41).is_err());
        // Statistics: k grows with the errors' sizes.
        let mut st = Stats::new(16384);
        assert_eq!((st.sum, st.k()), (256, 8));
        st.update(-300);
        assert_eq!((st.sum, st.count, st.k()), (556, 2, 9));
        assert_eq!(
            [bucket(0, 0), bucket(-17, 0), bucket(18, 0), bucket(300, 0)],
            [0, -1, 2, 4]
        );
        assert_eq!(bucket(2, 2), 0);
    }

    #[test]
    fn decodes_flat_stripes_of_both_sensors_and_depths() {
        let cancel = AtomicBool::new(false);
        for bits in [12, 14, 16] {
            for xtrans in [false, true] {
                // 780 columns: the second stripe's 12 columns are kept.
                let data = stream(bits, xtrans, 780, 3, &zeros(bits, xtrans, 3), None);
                let out = decode(&data, 780, 18, bits, xtrans, &cancel).unwrap();
                assert_eq!(out, vec![0; 780 * 18]);
            }
        }
    }

    #[test]
    fn lossy_groups_and_garbage_are_errors_or_samples_never_panics() {
        let cancel = AtomicBool::new(false);
        let data = stream(14, true, 780, 3, &zeros(14, true, 3), Some(&[0, 0, 0]));
        assert_eq!(
            decode(&data, 780, 18, 14, true, &cancel).unwrap(),
            vec![0; 780 * 18]
        );
        for len in (0..data.len()).step_by(5) {
            let _ = decode(&data[..len], 780, 18, 14, true, &cancel);
        }
        for at in 0..64 {
            let mut bad = data.clone();
            bad[at] ^= 0xa5;
            let _ = decode(&bad, 780, 18, 14, true, &cancel);
        }
        let mut seed = 0x1234_5678u32;
        let garbage: Vec<u8> = (0..4096)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 24) as u8
            })
            .collect();
        for q in [0u8, 1, 3, 9] {
            let data = stream(12, false, 780, 1, &garbage, Some(&[q]));
            let _ = decode(&data, 780, 6, 12, false, &cancel);
        }
        // Stripes too short for their samples are refused before the image
        // is allocated.
        let short = stream(14, true, 780, 3, &[0; 64], None);
        assert!(matches!(
            decode(&short, 780, 18, 14, true, &cancel),
            Err(Error::Corrupt(_))
        ));
        // The header must describe the image.
        assert!(decode(&data, 782, 18, 14, true, &cancel).is_err());
        assert!(matches!(
            decode(&data, 780, 18, 14, false, &cancel),
            Err(Error::Unsupported(_))
        ));
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            decode(&data, 780, 18, 14, true, &cancel),
            Err(Error::Cancelled)
        );
    }
}
