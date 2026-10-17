//! Sony's "compressed RAW 2" (ARW compression 32766): a strip of tiles,
//! each an LLVC picture (SMPTE RDD 34) of three wavelet planes (the green
//! mean and two chroma differences) and a residual plane of the greens'
//! difference, rebuilt into a mosaic of 12-bit codes.
//!
//! Sources: SMPTE RDD 34:2015 (Picture_info, transmission units, coefficient
//! coding, the wavelet; see `llvc`); our own measurements against LibRaw's
//! output on our files (the tile table, the stream and index layout of a
//! tile, the transmission units' rows and shift, the colour reconstruction)
//! and Adobe DNG Converter's output on a synthetic file holding every code
//! (the curve to linear values).
use super::llvc::{self, Half, Level, Plane};
use crate::error::{Error, Result, corrupt};
use std::sync::atomic::{AtomicBool, Ordering};

/// A tile of the strip: where its bytes start (in the strip), and where it
/// goes in the image.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tile {
    pub offset: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// The most tiles a strip holds.
const MAX_TILES: usize = 64;

fn u32_le(b: &[u8], at: usize) -> Option<usize> {
    let b = b.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn be(b: &[u8], at: usize, n: usize) -> Option<usize> {
    Some(
        b.get(at..at + n)?
            .iter()
            .fold(0, |a, &v| a << 8 | usize::from(v)),
    )
}

/// The tile table at the start of the strip: a count, a zero, then per tile
/// a 64-bit offset (in the strip) and x, y, width, height, little-endian.
pub(crate) fn tiles(strip: &[u8], width: usize, height: usize) -> Result<Vec<Tile>> {
    let n = u32_le(strip, 0).ok_or_else(|| corrupt("ARW6 tile table"))?;
    if n == 0 || n > MAX_TILES {
        return Err(corrupt("ARW6 tile count"));
    }
    let mut out: Vec<Tile> = Vec::with_capacity(n);
    for i in 0..n {
        let e = strip
            .get(8 + 24 * i..32 + 24 * i)
            .ok_or_else(|| corrupt("ARW6 tile table"))?;
        let offset = u64::from_le_bytes(e[..8].try_into().map_err(|_| corrupt("ARW6 tile"))?);
        let field = |k: usize| u32_le(e, 8 + 4 * k).unwrap_or(0);
        let t = Tile {
            offset: usize::try_from(offset).map_err(|_| corrupt("ARW6 tile offset"))?,
            x: field(0),
            y: field(1),
            width: field(2),
            height: field(3),
        };
        let fits = t.offset < strip.len()
            && out.last().is_none_or(|p| t.offset > p.offset)
            && t.width > 0
            && t.height > 0
            && t.width.is_multiple_of(16)
            && t.height.is_multiple_of(2)
            && t.x.checked_add(t.width).is_some_and(|r| r <= width)
            && t.y.checked_add(t.height).is_some_and(|b| b <= height);
        if !fits {
            return Err(corrupt("ARW6 tile"));
        }
        out.push(t);
    }
    Ok(out)
}

/// A stream of a tile: its group (0 the lowest band, 1–3 levels 3 to 1, 4 the
/// residual), and where its index and data are (bytes in the tile).
struct Stream {
    group: usize,
    index: usize,
    entries: usize,
    data: usize,
    end: usize,
}

/// The streams a group holds: one per component, the residual's one.
const GROUP_STREAMS: [usize; 5] = [3, 3, 3, 3, 1];

/// A tile's picture size (HS, VS: the planes' width and height) and its 13
/// streams: word 0 is RDD 34's Picture_info, words 3 to 7 each group's
/// stream sizes (in 16-byte words), the streams from word 8 on, each a word
/// of its index's and data's sizes, the index, then the data.
fn streams(tile: &[u8], width: usize, height: usize) -> Result<(usize, Vec<Stream>)> {
    let word0 = tile.get(..16).ok_or_else(|| corrupt("ARW6 tile header"))?;
    let bits = |pos: usize, n: usize| {
        (pos..pos + n).fold(0usize, |a, i| {
            a << 1 | usize::from(word0[i / 8] >> (7 - i % 8) & 1)
        })
    };
    let (hs, vs, depth, nc, nw, np) = (
        bits(64, 16),
        bits(80, 16),
        bits(102, 6),
        bits(112, 3),
        bits(115, 3),
        bits(118, 10),
    );
    if hs != width || 2 * vs != height || depth != 16 || nc != 3 || nw != 3 || np != 0b10_0000_0000
    {
        return Err(Error::Unsupported("ARW6 picture header".into()));
    }
    let mut out = Vec::with_capacity(13);
    let mut word = 8;
    for (group, &n) in GROUP_STREAMS.iter().enumerate() {
        let table = tile
            .get((3 + group) * 16..(4 + group) * 16)
            .ok_or_else(|| corrupt("ARW6 group table"))?;
        if usize::from(table[0]) != n {
            return Err(Error::Unsupported("ARW6 group streams".into()));
        }
        for k in 0..n {
            let size = be(table, 1 + 3 * k, 3).ok_or_else(|| corrupt("ARW6 group table"))?;
            let head = tile
                .get(word * 16..word * 16 + 16)
                .ok_or_else(|| corrupt("ARW6 stream"))?;
            let (index, data) = (be(head, 0, 2).unwrap_or(0), be(head, 2, 3).unwrap_or(0));
            if index + data + 1 > size || (word + size) * 16 > tile.len() {
                return Err(corrupt("ARW6 stream sizes"));
            }
            out.push(Stream {
                group,
                index: (word + 1) * 16,
                entries: index * 16,
                data: (word + 1 + index) * 16,
                end: (word + 1 + index + data) * 16,
            });
            word += size;
        }
    }
    Ok((vs, out))
}

/// A half transmission unit of a stream: its bytes and quantisation indices
/// (one for the lowest band and the residual; HL, LH, HH for the others).
struct Part<'a> {
    bytes: &'a [u8],
    qi: [u32; 3],
}

/// A stream's halves: each index entry gives two halves' byte lengths
/// (16 bits) each followed by its quantisation nibbles; the halves' bytes
/// follow one another in the data.
fn halves<'a>(tile: &'a [u8], s: &Stream, count: usize) -> Result<Vec<Part<'a>>> {
    let nx = if s.group == 0 || s.group == 4 { 1 } else { 3 };
    let size = 4 + nx;
    let index = tile
        .get(s.index..s.index + s.entries)
        .ok_or_else(|| corrupt("ARW6 index"))?;
    let nibble = |e: &[u8], i: usize| u32::from(e[i / 2] >> (4 * (1 - i % 2)) & 15);
    let mut out = Vec::with_capacity(count);
    let mut at = s.data;
    for k in 0..count.div_ceil(2) {
        let e = index
            .get(k * size..(k + 1) * size)
            .ok_or_else(|| corrupt("ARW6 index entries"))?;
        for h in 0..2 {
            let base = h * (4 + nx);
            let len = (0..4).fold(0usize, |a, i| a << 4 | nibble(e, base + i) as usize);
            let q: Vec<u32> = (0..nx).map(|i| nibble(e, base + 4 + i)).collect();
            let qi = if nx == 1 {
                [q[0]; 3]
            } else {
                [q[0], q[1], q[2]]
            };
            let bytes = tile
                .get(at..at + len)
                .filter(|_| at + len <= s.end)
                .ok_or_else(|| corrupt("ARW6 half"))?;
            out.push(Part { bytes, qi });
            at += len;
        }
    }
    out.truncate(count);
    Ok(out)
}

/// Where half `n`'s rows go: its plane rows, and those rows' places in the
/// bands of each level (low and high).
struct Rows {
    plane: Vec<usize>,
    low: [Vec<usize>; 3],
    high: [Vec<usize>; 3],
}

/// The plane rows of half `n`: `8n − 6 .. 8n + 2`, clipped (our
/// measurements), split into each level's bands by the levels' phases.
fn rows(n: usize, shift: usize, vs: usize, phases: [usize; 3]) -> Rows {
    let start = (8 * n + shift).saturating_sub(6).min(vs);
    let plane: Vec<usize> = (start..(8 * n + shift + 2).min(vs)).collect();
    let mut low: [Vec<usize>; 3] = Default::default();
    let mut high: [Vec<usize>; 3] = Default::default();
    let mut cur = plane.clone();
    for (level, &p) in phases.iter().enumerate() {
        for &t in &cur {
            if t % 2 == p {
                low[level].push((t - p) / 2);
            } else {
                high[level].push((t + p - 1) / 2);
            }
        }
        cur = low[level].clone();
    }
    Rows { plane, low, high }
}

/// The phases of the three levels (which rows are low-pass) of a tile whose
/// transmission units are shifted `shift` rows: the full sensor's are
/// [0, 1, 1], the tile starting `8 − shift` plane rows (modulo 8) into it.
fn phases(shift: usize) -> [usize; 3] {
    let o = (8 - shift % 8) % 8;
    let p1 = o % 2;
    let k1 = (o + p1) / 2;
    let p2 = (1 + k1) % 2;
    let k2 = (p2 + k1).saturating_sub(1) / 2;
    let p3 = (1 + k2) % 2;
    [p1, p2, p3]
}

/// The rows of the first half transmission unit beyond two: how far the
/// units are shifted. Residual lines are counted while a byte remains.
fn shift(tile: &[u8], residual: &Stream, width: usize) -> Result<usize> {
    let first = halves(tile, residual, 1)?;
    let mut half = Half::new(first[0].bytes);
    let mut lines = 0;
    while lines < 8 && half.len().saturating_sub(half.consumed()) >= 8 {
        llvc::line(&mut half, width)?;
        lines += 1;
    }
    match lines {
        2..=7 => Ok(lines - 2),
        _ => Err(Error::Unsupported("ARW6 transmission unit rows".into())),
    }
}

/// Decodes one tile into its four planes: the green mean, two chroma and the
/// residual, `width / 2 × height / 2` each.
fn planes(tile: &[u8], width: usize, height: usize, cancel: &AtomicBool) -> Result<[Plane; 4]> {
    let (vs, streams) = streams(tile, width, height)?;
    let w = width / 2;
    if !w.is_multiple_of(8) || vs == 0 {
        return Err(Error::Unsupported("ARW6 tile size".into()));
    }
    let shift = shift(tile, &streams[12], w)?;
    let count = vs.saturating_sub(2 + shift).div_ceil(8) + 1;
    let ph = phases(shift);
    let layout: Vec<Rows> = (0..count).map(|n| rows(n, shift, vs, ph)).collect();
    let (n1, h1) = llvc::split(vs, ph[0]);
    let (n2, h2) = llvc::split(n1, ph[1]);
    let (n3, h3) = llvc::split(n2, ph[2]);
    let (w1, w2, w3) = (w >> 1, w >> 2, w >> 3);
    let mut out: [Plane; 4] = Default::default();
    for c in 0..3 {
        let z = Plane::zeros;
        let mut bands = [
            z(w3, n3), // LL3
            z(w3, n3), // HL3
            z(w3, h3), // LH3
            z(w3, h3), // HH3
            z(w2, n2), // HL2
            z(w2, h2), // LH2
            z(w2, h2), // HH2
            z(w1, n1), // HL1
            z(w1, h1), // LH1
            z(w1, h1), // HH1
        ];
        for g in 0..4 {
            let s = &streams[3 * g + c];
            let width = [w3, w3, w2, w1][g];
            for (part, r) in halves(tile, s, count)?.iter().zip(&layout) {
                // Every half transmission unit: a few lines.
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let mut half = Half::new(part.bytes);
                // Which bands the half's lines fill, in order: the lowest
                // band, or a level's HL (its low rows), LH and HH (high rows).
                let targets: Vec<(usize, &Vec<usize>, u32)> = match g {
                    0 => vec![(0, &r.low[2], 0)],
                    _ => {
                        let level = 3 - g;
                        let first = [7, 4, 1][level];
                        vec![
                            (first, &r.low[level], part.qi[0]),
                            (first + 1, &r.high[level], part.qi[1]),
                            (first + 2, &r.high[level], part.qi[2]),
                        ]
                    }
                };
                for (band, band_rows, qi) in targets {
                    for &row in band_rows {
                        let line = llvc::line(&mut half, width)?;
                        let dst = bands[band].row_mut(row);
                        if g == 0 {
                            // The lowest band's lines are differences along
                            // the line, from 2048 for the green mean.
                            let mut acc: i32 = if c == 0 { 2048 } else { 0 };
                            for (d, v) in dst.iter_mut().zip(&line) {
                                acc = acc.wrapping_add(*v);
                                *d = acc;
                            }
                        } else {
                            for (d, &v) in dst.iter_mut().zip(&line) {
                                *d = llvc::dequantise(v, qi);
                            }
                        }
                    }
                }
            }
        }
        let [ll3, hl3, lh3, hh3, hl2, lh2, hh2, hl1, lh1, hh1] = &bands;
        let ll2 = llvc::inverse(
            &Level {
                ll: ll3,
                lh: lh3,
                hl: hl3,
                hh: hh3,
            },
            ph[2],
            cancel,
        )?;
        let ll1 = llvc::inverse(
            &Level {
                ll: &ll2,
                lh: lh2,
                hl: hl2,
                hh: hh2,
            },
            ph[1],
            cancel,
        )?;
        out[c] = llvc::inverse(
            &Level {
                ll: &ll1,
                lh: lh1,
                hl: hl1,
                hh: hh1,
            },
            ph[0],
            cancel,
        )?;
    }
    let mut res = Plane::zeros(w, vs);
    for (part, r) in halves(tile, &streams[12], count)?.iter().zip(&layout) {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let mut half = Half::new(part.bytes);
        for &row in &r.plane {
            let line = llvc::line(&mut half, w)?;
            for (d, &v) in res.row_mut(row).iter_mut().zip(&line) {
                *d = llvc::dequantise(v, part.qi[0]);
            }
        }
    }
    out[3] = res;
    Ok(out)
}

/// The tile's mosaic of 12-bit codes from its planes: the first green is the
/// mean less a quarter of the residual around it, the second the first's
/// average around it plus the residual, red and blue twice their chroma
/// plus the greens' mean (our measurements). Red, green / green, blue cells.
fn mosaic(p: &[Plane; 4], out: &mut [u16], stride: usize, cancel: &AtomicBool) -> Result<()> {
    let [m, c1, c2, res] = p;
    let (w, h) = (m.width, m.height);
    let mut g1 = vec![0i64; w * h];
    for j in 0..h {
        if j % 16 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let (rp, rj) = (res.row(j.saturating_sub(1)), res.row(j));
        let mj = m.row(j);
        for x in 0..w {
            let xn = (x + 1).min(w - 1);
            let r = i64::from(rp[x]) + i64::from(rp[xn]) + i64::from(rj[x]) + i64::from(rj[xn]);
            g1[j * w + x] = i64::from(mj[x]) - ((r + 4) >> 3);
        }
    }
    let clip = |v: i64| v.clamp(0, 4095) as u16;
    for j in 0..h {
        if j % 16 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let jn = (j + 1).min(h - 1);
        for x in 0..w {
            let xp = x.saturating_sub(1);
            let g = |jj: usize, xx: usize| g1[jj * w + xx];
            let first = g(j, x);
            let second =
                ((g(j, xp) + g(j, x) + g(jn, xp) + g(jn, x)) >> 2) + i64::from(res.row(j)[x]);
            let mean = (first.clamp(0, 4095) + second.clamp(0, 4095)) >> 1;
            let top = 2 * j * stride + 2 * x;
            out[top] = clip(2 * i64::from(c1.row(j)[x]) + mean);
            out[top + 1] = clip(first);
            out[top + stride] = clip(second);
            out[top + stride + 1] = clip(2 * i64::from(c2.row(j)[x]) + mean);
        }
    }
    Ok(())
}

/// Decodes the strip into `width × height` 12-bit codes.
pub(crate) fn decode(
    strip: &[u8],
    width: usize,
    height: usize,
    out: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let tiles = tiles(strip, width, height)?;
    for (i, t) in tiles.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let end = tiles.get(i + 1).map_or(strip.len(), |n| n.offset);
        let bytes = &strip[t.offset..end];
        let p = planes(bytes, t.width, t.height, cancel)?;
        mosaic(&p, &mut out[t.y * width + t.x..], width, cancel)?;
    }
    Ok(())
}

/// Sony's curve from 12-bit codes to linear values (twice the 14-bit
/// scale), every eighth code from 1408 to 4096; below 1408 the identity.
const KNOTS: [u16; 337] = [
    1408, 1416, 1424, 1433, 1442, 1450, 1459, 1467, 1475, 1484, 1493, 1501, 1509, 1517, 1526, 1536,
    1545, 1555, 1566, 1575, 1585, 1595, 1606, 1615, 1626, 1636, 1646, 1656, 1667, 1678, 1691, 1702,
    1715, 1725, 1737, 1749, 1760, 1773, 1784, 1797, 1811, 1824, 1837, 1851, 1864, 1878, 1891, 1904,
    1918, 1933, 1947, 1963, 1977, 1992, 2007, 2022, 2036, 2052, 2068, 2084, 2100, 2117, 2133, 2149,
    2165, 2184, 2201, 2219, 2237, 2256, 2274, 2292, 2311, 2331, 2349, 2369, 2388, 2407, 2427, 2448,
    2469, 2491, 2511, 2533, 2553, 2575, 2598, 2620, 2643, 2666, 2689, 2713, 2736, 2761, 2785, 2810,
    2835, 2860, 2886, 2912, 2938, 2965, 2992, 3019, 3047, 3075, 3103, 3132, 3160, 3190, 3219, 3250,
    3280, 3311, 3343, 3373, 3405, 3438, 3471, 3503, 3537, 3571, 3606, 3641, 3675, 3711, 3747, 3784,
    3820, 3858, 3896, 3934, 3973, 4012, 4052, 4092, 4133, 4174, 4216, 4258, 4301, 4344, 4388, 4432,
    4478, 4522, 4569, 4615, 4662, 4710, 4758, 4808, 4856, 4907, 4958, 5008, 5060, 5113, 5166, 5220,
    5274, 5330, 5386, 5442, 5499, 5557, 5616, 5675, 5735, 5797, 5858, 5920, 5983, 6047, 6111, 6177,
    6243, 6310, 6378, 6446, 6516, 6587, 6657, 6730, 6803, 6877, 6951, 7027, 7103, 7181, 7259, 7339,
    7419, 7501, 7584, 7667, 7751, 7836, 7923, 8010, 8099, 8189, 8279, 8372, 8464, 8559, 8654, 8750,
    8848, 8947, 9047, 9148, 9251, 9355, 9460, 9566, 9674, 9782, 9893, 10005, 10117, 10232, 10348,
    10465, 10583, 10703, 10825, 10948, 11072, 11199, 11326, 11455, 11586, 11718, 11852, 11988,
    12125, 12264, 12405, 12547, 12691, 12837, 12984, 13134, 13285, 13438, 13593, 13750, 13909,
    14070, 14232, 14397, 14564, 14732, 14903, 15076, 15251, 15428, 15607, 15789, 15972, 16158,
    16347, 16537, 16730, 16925, 17123, 17323, 17525, 17730, 17938, 18148, 18360, 18575, 18793,
    19014, 19236, 19462, 19691, 19922, 20157, 20394, 20634, 20877, 21123, 21371, 21623, 21878,
    22136, 22398, 22662, 22930, 23201, 23475, 23753, 24034, 24318, 24606, 24898, 25193, 25491,
    25794, 26100, 26410, 26723, 27040, 27362, 27687, 28016, 28349, 28686, 29028, 29373, 29723,
    30077, 30435, 30798, 31165, 31537, 31913, 32294, 32679, 33069, 33464, 33864, 34269, 34678,
    35093, 35513, 35937, 36367, 36803, 37243, 37689, 38141, 38597, 39060,
];

/// The linear value of every 12-bit code: the identity to 1408, then
/// [`KNOTS`] interpolated (rounding half up) over each eight codes, and the
/// 14-bit white doubled at code 3977 (Adobe DNG Converter 18.7's values for
/// every code, our measurements of its output on a synthetic file; LibRaw's
/// agree on every code our files hold).
pub(crate) fn curve() -> Vec<u16> {
    (0..4096u32)
        .map(|c| {
            if c < 1408 {
                return c as u16;
            }
            if c == 3977 {
                return 32766;
            }
            let (i, j) = (((c - 1408) / 8) as usize, (c % 8));
            let (a, b) = (u32::from(KNOTS[i]), u32::from(KNOTS[i + 1]));
            (a + (2 * j * (b - a) + 8) / 16) as u16
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bits, most significant first.
    #[derive(Default)]
    pub(crate) struct Writer {
        bits: Vec<bool>,
    }

    impl Writer {
        fn put(&mut self, v: u64, n: u32) {
            for i in (0..n).rev() {
                self.bits.push(v >> i & 1 == 1);
            }
        }
        fn bytes(&self) -> Vec<u8> {
            self.bits
                .chunks(8)
                .map(|c| c.iter().fold(0u8, |a, &x| a << 1 | u8::from(x)) << (8 - c.len()))
                .collect()
        }
    }

    /// Codes a line as RDD 34's figures 6.5–6.8 read it, written from the
    /// figures independently of the decoder: a zero line as a zero set and a
    /// run to its end; any other with every set at the depth of its largest
    /// magnitude, signs after the following set's depth code.
    pub(crate) fn encode_line(w: &mut Writer, v: &[i32]) {
        let sets = v.len().div_ceil(4);
        let depth = v
            .iter()
            .map(|x| 32 - x.unsigned_abs().leading_zeros())
            .max()
            .unwrap_or(0);
        if depth == 0 {
            w.put(0, 1);
            if sets > 1 {
                w.put(0, sets.next_power_of_two().trailing_zeros());
            }
            return;
        }
        let set = |k: usize| -> [i32; 4] {
            std::array::from_fn(|i| v.get(4 * k + i).copied().unwrap_or(0))
        };
        for k in 0..=sets {
            if k == 0 {
                // "1 0 0^n 1": the depth up from 0 by n + 1.
                w.put(0b10, 2);
                w.put(0, depth - 1);
                w.put(1, 1);
            } else if k < sets {
                // "0": the same depth.
                w.put(0, 1);
            }
            if k > 0 {
                for x in set(k - 1) {
                    if x != 0 {
                        w.put(u64::from(x < 0), 1);
                    }
                }
            }
            if k < sets {
                for x in set(k) {
                    w.put(u64::from(x.unsigned_abs()), depth);
                }
            }
        }
    }

    /// A tile of `width × height` whose green mean's lowest band is `ll3`
    /// (`width / 16` wide), every other band, chroma and the residual zero,
    /// laid out as `planes` reads tiles: Picture_info, group tables, then per
    /// stream its header, index and halves. Transmission units unshifted.
    pub(crate) fn encode_tile(width: usize, height: usize, ll3: &Plane) -> Vec<u8> {
        let (w, vs) = (width / 2, height / 2);
        let count = vs.saturating_sub(2).div_ceil(8) + 1;
        let ph = phases(0);
        let layout: Vec<Rows> = (0..count).map(|n| rows(n, 0, vs, ph)).collect();
        let (w1, w2, w3) = (w >> 1, w >> 2, w >> 3);
        let mut streams = vec![];
        for (g, &n) in GROUP_STREAMS.iter().enumerate() {
            for c in 0..n {
                let mut halves = vec![];
                for r in &layout {
                    let mut wr = Writer::default();
                    let zero = |wr: &mut Writer, width: usize, rows: usize| {
                        for _ in 0..rows {
                            encode_line(wr, &vec![0; width]);
                        }
                    };
                    match g {
                        0 => {
                            for &row in &r.low[2] {
                                let mut prev = if c == 0 { 2048 } else { 0 };
                                let line: Vec<i32> = (0..w3)
                                    .map(|x| {
                                        let v = if c == 0 { ll3.data[row * w3 + x] } else { 0 };
                                        let d = v - prev;
                                        prev = v;
                                        d
                                    })
                                    .collect();
                                encode_line(&mut wr, &line);
                            }
                        }
                        4 => zero(&mut wr, w, r.plane.len()),
                        _ => {
                            let level = 3 - g;
                            let width = [w1, w2, w3][level];
                            zero(&mut wr, width, r.low[level].len() + 2 * r.high[level].len());
                        }
                    }
                    halves.push(wr.bytes());
                }
                let nx = if g == 0 || g == 4 { 1 } else { 3 };
                let mut index = vec![];
                for pair in halves.chunks(2) {
                    let mut e = Writer::default();
                    for h in pair {
                        e.put(h.len() as u64, 16);
                        e.put(0, 4 * nx);
                    }
                    index.extend(e.bytes());
                }
                let mut data: Vec<u8> = halves.concat();
                index.resize(index.len().div_ceil(16) * 16, 0);
                data.resize(data.len().div_ceil(16) * 16, 0);
                let mut s = vec![0u8; 16];
                s[..2].copy_from_slice(&((index.len() / 16) as u16).to_be_bytes());
                s[2..5].copy_from_slice(&((data.len() / 16) as u32).to_be_bytes()[1..]);
                s.extend(index);
                s.extend(data);
                streams.push((g, s));
            }
        }
        let mut tile = vec![0u8; 8 * 16];
        let mut word0 = Writer::default();
        word0.put(0, 64);
        word0.put(width as u64, 16);
        word0.put(vs as u64, 16);
        word0.put(0, 6);
        word0.put(16, 6);
        word0.put(0, 4);
        word0.put(3, 3);
        word0.put(3, 3);
        word0.put(0b10_0000_0000, 10);
        tile[..16].copy_from_slice(&word0.bytes());
        for g in 0..5 {
            let sizes: Vec<usize> = streams
                .iter()
                .filter(|s| s.0 == g)
                .map(|s| s.1.len() / 16)
                .collect();
            let t = &mut tile[(3 + g) * 16..(4 + g) * 16];
            t[0] = sizes.len() as u8;
            for (k, size) in sizes.iter().enumerate() {
                t[1 + 3 * k..4 + 3 * k].copy_from_slice(&(*size as u32).to_be_bytes()[1..]);
            }
            let total: usize = sizes.iter().sum::<usize>() * 16;
            tile[16 + 3 * g..19 + 3 * g].copy_from_slice(&(total as u32).to_be_bytes()[1..]);
        }
        for (_, s) in streams {
            tile.extend(s);
        }
        tile
    }

    /// A strip of one tile.
    pub(crate) fn encode_strip(width: usize, height: usize, tile: &[u8]) -> Vec<u8> {
        let mut strip = vec![0u8; 512];
        strip[..4].copy_from_slice(&1u32.to_le_bytes());
        strip[8..16].copy_from_slice(&512u64.to_le_bytes());
        strip[24..28].copy_from_slice(&(width as u32).to_le_bytes());
        strip[28..32].copy_from_slice(&(height as u32).to_le_bytes());
        strip.extend(tile);
        strip
    }

    #[test]
    fn decodes_tiles_of_the_lowest_band() {
        // A 256 × 64 tile: a 16 × 4 lowest band of flat 3 × 3 blocks gives
        // their values at the blocks' centres; the mosaic is that value in
        // every colour where nothing else is coded.
        let (width, height) = (256, 64);
        let (w3, h3) = (16, 4);
        let ll3 = Plane {
            width: w3,
            height: h3,
            data: (0..w3 * h3)
                .map(|i| 1000 + (i % w3 / 3) as i32 * 300)
                .collect(),
        };
        let strip = encode_strip(width, height, &encode_tile(width, height, &ll3));
        let mut out = vec![0u16; width * height];
        decode(&strip, width, height, &mut out, &AtomicBool::new(false)).unwrap();
        // The centre of the block of 1300 (lowest-band columns 3 to 5) is
        // mosaic column 4 × 16 + 8.
        for row in [16, 17, 32, 33] {
            assert_eq!(out[row * width + 4 * 16 + 8], 1300, "row {row}");
        }
        assert_eq!(out[33 * width + 16 + 8], 1000);
    }

    #[test]
    fn the_curve_is_the_identity_then_interpolates_eighths() {
        let c = curve();
        assert_eq!(&c[..3], &[0, 1, 2]);
        assert_eq!(c[1407], 1407);
        // Knots 1408, 1416, 1424 and 1433: 1428 is halfway from 1424 to 1433.
        assert_eq!(
            &c[1408..1417],
            &[1408, 1409, 1410, 1411, 1412, 1413, 1414, 1415, 1416]
        );
        assert_eq!(c[1428], 1424 + (2 * 4 * 9 + 8) / 16);
        assert_eq!(c[3977], 32766);
        assert_eq!(c[4095], 39002);
        assert!(c.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn cancelling_stops_a_decode_soon() {
        // A 4096 × 4096 tile, cancelled from another thread while it decodes.
        let (width, height) = (4096, 4096);
        let ll3 = Plane {
            width: width / 16,
            height: llvc::split(llvc::split(llvc::split(height / 2, 0).0, 1).0, 1).0,
            data: vec![1500; width / 16 * 256],
        };
        let strip = encode_strip(width, height, &encode_tile(width, height, &ll3));
        let cancel = AtomicBool::new(false);
        let mut out = vec![0u16; width * height];
        let (result, latency) = std::thread::scope(|s| {
            let decoding = s.spawn(|| {
                let r = decode(&strip, width, height, &mut out, &cancel);
                (r, std::time::Instant::now())
            });
            std::thread::sleep(std::time::Duration::from_millis(20));
            let at = std::time::Instant::now();
            cancel.store(true, Ordering::Relaxed);
            let (r, done) = decoding.join().unwrap();
            (r, done.saturating_duration_since(at))
        });
        assert_eq!(result, Err(Error::Cancelled));
        eprintln!("cancelled after {latency:?}");
        assert!(
            latency < std::time::Duration::from_millis(500),
            "{latency:?}"
        );
    }

    #[test]
    fn truncated_and_garbage_strips_are_errors_not_panics() {
        let (width, height) = (128, 32);
        let ll3 = Plane {
            width: 8,
            height: 2,
            data: (0..16).map(|i| 900 + i * 37).collect(),
        };
        let strip = encode_strip(width, height, &encode_tile(width, height, &ll3));
        let mut out = vec![0u16; width * height];
        let cancel = AtomicBool::new(false);
        for end in (0..strip.len()).step_by(7) {
            let _ = decode(&strip[..end], width, height, &mut out, &cancel);
        }
        let mut noisy = strip.clone();
        for (i, b) in noisy.iter_mut().enumerate().skip(16) {
            *b ^= (i * 7919 % 251) as u8;
        }
        let _ = decode(&noisy, width, height, &mut out, &cancel);
    }
}
