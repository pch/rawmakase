//! Canon's CRX coding of CR3 sensor data, from bytes to samples.
//!
//! The image header (the CR3 track's `CMP1` box) gives the image and tile
//! sizes, the bit depth, the number of colour planes (four: the Bayer cell's
//! sites), the number of wavelet levels (0 lossless, 3 for C-RAW) and the
//! size of the tile headers at the start of the coded data. Those headers
//! are a sequence of markers, big-endian: per tile (`FF01`, or `FF11` in
//! version 2 of the coding) its size, per plane (`FF02`/`FF12`) its size and
//! flags, and per subband (`FF03`/`FF13`) its size and flags, the
//! quantization included; the coded data follows them in the same order.
//! Version 2 tiles start with a map of quantization parameters.
//!
//! Each plane of a tile is a half-width, half-height image coded on its own:
//! - Lossless planes, and the low-pass band of C-RAW planes, are predicted
//!   from the median of the left, upper and upper-left neighbours (as JPEG-LS,
//!   ITU T.87, predicts) and the residuals Golomb-Rice coded with a parameter
//!   adapted to the previous residual and the upper row's gradient. Where the
//!   left and the two upper neighbours agree, a flag starts a run of the left
//!   value whose length is coded as in JPEG-LS's run mode; a run ending inside
//!   the row is followed by one residual predicted from above.
//! - C-RAW's high-pass subbands code their coefficients the same way with
//!   zero runs, then are dequantized and recombined with the integer 5/3
//!   wavelet of JPEG 2000 (ITU T.800 Annex F), over the tile's neighbours'
//!   columns at its inner edges.
//!
//! A sample is the plane's value plus half the bit depth's range.
//!
//! Sources: Laurent Clévy's public description of the CR3 format (the
//! `CMP1` fields and the marker layout), ITU T.87 (median prediction, the
//! run-mode index table), ITU T.800 Annex F (the 5/3 lifting), ITU-T H.265
//! (the quantization step scale), and our own measurements.
use crate::codec::bits::Msb;
use crate::error::{Error, Result, corrupt};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The image header (`CMP1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub version: u16,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub bits: u8,
    pub planes: u8,
    /// The site of red in the Bayer cell: 0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR.
    pub cfa: u8,
    pub encoding: u8,
    pub levels: u8,
    /// The bytes of tile headers before the coded data.
    pub header_size: u32,
}

/// The most tiles and the largest tile header read.
const MAX_TILES: usize = 1024;
const MAX_HEADER: u32 = 1 << 20;

impl Header {
    /// The header in a `CMP1` box's payload.
    pub fn parse(p: &[u8]) -> Result<Header> {
        let u16_at = |at: usize| p.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let u32_at = |at: usize| {
            p.get(at..at + 4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        let byte = |at: usize| p.get(at).copied();
        let short = || corrupt("CR3 image header");
        let h = Header {
            version: u16_at(4).ok_or_else(short)?,
            width: u32_at(8).ok_or_else(short)?,
            height: u32_at(12).ok_or_else(short)?,
            tile_width: u32_at(16).ok_or_else(short)?,
            tile_height: u32_at(20).ok_or_else(short)?,
            bits: byte(24).ok_or_else(short)?,
            planes: byte(25).ok_or_else(short)? >> 4,
            cfa: byte(25).ok_or_else(short)? & 15,
            encoding: byte(26).ok_or_else(short)? >> 4,
            levels: byte(26).ok_or_else(short)? & 15,
            header_size: u32_at(28).ok_or_else(short)?,
        };
        Ok(h)
    }
    /// Whether this coding is decoded here.
    pub fn check(&self) -> Result<()> {
        if !matches!(self.version, 0x100 | 0x200)
            || self.planes != 4
            || self.encoding != 0
            || !matches!(self.levels, 0 | 3)
            || self.cfa > 3
            || !(8..=16).contains(&self.bits)
        {
            return Err(Error::Unsupported(format!(
                "CRX version {:#x}, {} planes, encoding {}, {} levels, {} bits",
                self.version, self.planes, self.encoding, self.levels, self.bits
            )));
        }
        crate::io::samples(self.width, self.height, 1)?;
        if self.tile_width == 0
            || self.tile_height == 0
            || self.tile_width % 2 == 1
            || self.tile_height % 2 == 1
            || self.width % 2 == 1
            || self.height % 2 == 1
        {
            return Err(corrupt("CRX tile size"));
        }
        if self.header_size > MAX_HEADER {
            return Err(Error::TooLarge("CRX tile headers"));
        }
        let tiles = u64::from(self.width.div_ceil(self.tile_width))
            * u64::from(self.height.div_ceil(self.tile_height));
        if tiles > MAX_TILES as u64 {
            return Err(Error::TooLarge("CRX tiles"));
        }
        Ok(())
    }
}

/// How a subband's coefficients are scaled back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quant {
    /// One quantization parameter for the whole band.
    Fixed(u32),
    /// Version 2: `base` plus `gain` eighths of the step of the tile's map.
    Adaptive { base: u32, gain: u32 },
}

#[derive(Clone, Debug)]
struct Band {
    /// Where the band's coded data is in the sample.
    start: usize,
    end: usize,
    quant: Quant,
}

#[derive(Clone, Debug)]
struct Tile {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    /// Each plane's subbands: the low-pass band, then per level from the
    /// coarsest the horizontally, vertically and diagonally high-pass ones.
    planes: Vec<Vec<Band>>,
    /// Version 2: where the quantization map is.
    map: Option<(usize, usize)>,
}

/// Reads a big-endian value of `n` bytes at `at`.
fn be(b: &[u8], at: usize, n: usize) -> Result<u32> {
    let s = b
        .get(at..at.checked_add(n).ok_or_else(|| corrupt("CRX header"))?)
        .ok_or_else(|| corrupt("CRX tile header"))?;
    Ok(s.iter().fold(0, |v, &b| (v << 8) | u32::from(b)))
}

/// The tiles, planes and subbands the tile headers describe.
fn layout(h: &Header, sample: &[u8]) -> Result<Vec<Tile>> {
    let head = sample
        .get(..h.header_size as usize)
        .ok_or_else(|| corrupt("CRX tile headers past the data"))?;
    let v2 = h.version == 0x200;
    let (width, height) = (h.width as usize, h.height as usize);
    let (tw, th) = (h.tile_width as usize, h.tile_height as usize);
    let across = width.div_ceil(tw);
    let count = across * height.div_ceil(th);
    let bands = 1 + 3 * usize::from(h.levels);
    let mut at = 0;
    // A marker and its length.
    let mut marker = |want: u16| -> Result<(usize, usize)> {
        let m = be(head, at, 2)?;
        let len = be(head, at + 2, 2)? as usize;
        if m != u32::from(want) {
            return Err(corrupt(format!("CRX marker {m:#x}, not {want:#x}")));
        }
        let body = at + 4;
        at = body + len;
        if at > head.len() {
            return Err(corrupt("CRX tile header past its end"));
        }
        Ok((body, len))
    };
    let mut data = h.header_size as usize;
    let mut tiles = Vec::with_capacity(count);
    for t in 0..count {
        let (body, len) = marker(if v2 { 0xff11 } else { 0xff01 })?;
        if len < 8 || (v2 && len < 16) {
            return Err(corrupt("CRX tile header"));
        }
        let size = be(head, body, 4)? as usize;
        let tile_end = data
            .checked_add(size)
            .filter(|&e| e <= sample.len())
            .ok_or_else(|| corrupt("CRX tile past the data"))?;
        let map = if v2 {
            // The quantization map, then padding.
            let len = be(head, body + 8, 4)? as usize;
            let pad = be(head, body + 12, 2)? as usize;
            let end = data + len;
            let next = end + pad;
            if next > tile_end {
                return Err(corrupt("CRX quantization map past its tile"));
            }
            let map = (data, end);
            data = next;
            Some(map)
        } else {
            None
        };
        let mut planes = Vec::with_capacity(4);
        for _ in 0..h.planes {
            let (body, _) = marker(if v2 { 0xff12 } else { 0xff02 })?;
            let size = be(head, body, 4)? as usize;
            let plane_end = data
                .checked_add(size)
                .filter(|&e| e <= tile_end)
                .ok_or_else(|| corrupt("CRX plane past its tile"))?;
            let mut list = Vec::with_capacity(bands);
            for _ in 0..bands {
                let (body, len) = marker(if v2 { 0xff13 } else { 0xff03 })?;
                let size = be(head, body, 4)? as usize;
                let flags = be(head, body + 4, 4)?;
                let end = data
                    .checked_add(size)
                    .filter(|&e| e <= plane_end)
                    .ok_or_else(|| corrupt("CRX subband past its plane"))?;
                let quant = if v2 {
                    if len < 16 {
                        return Err(corrupt("CRX subband header"));
                    }
                    Quant::Adaptive {
                        base: be(head, body + 8, 4)?,
                        gain: flags & 0x07ff_ffff,
                    }
                } else {
                    Quant::Fixed((flags >> 19) & 0xff)
                };
                list.push(Band {
                    start: data,
                    end,
                    quant,
                });
                data = end;
            }
            data = plane_end;
            planes.push(list);
        }
        data = tile_end;
        let (left, top) = ((t % across) * tw, (t / across) * th);
        tiles.push(Tile {
            left,
            top,
            width: tw.min(width - left),
            height: th.min(height - top),
            planes,
            map,
        });
    }
    Ok(tiles)
}

/// Decodes the sensor data in `sample`, which the image header `h` describes:
/// `width × height` samples.
pub(crate) fn decode(h: &Header, sample: &[u8], cancel: &AtomicBool) -> Result<Vec<u16>> {
    h.check()?;
    if h.levels > 0 && h.tile_height < h.height {
        return Err(Error::Unsupported("C-RAW tiles stacked vertically".into()));
    }
    let tiles = layout(h, sample)?;
    let width = h.width as usize;
    let count = crate::io::samples(h.width, h.height, 1)?;
    // Allocated once the first tile decodes: a small file can claim a large
    // image.
    let mut out: Vec<u16> = Vec::new();
    let mid = 1i32 << (h.bits - 1);
    let max = (1i32 << h.bits) - 1;
    for tile in &tiles {
        if tile.width % 2 == 1 || tile.height % 2 == 1 {
            return Err(corrupt("CRX tile of odd size"));
        }
        let (pw, ph) = (tile.width / 2, tile.height / 2);
        let edges = Edges {
            before: tile.left > 0,
            after: tile.left + tile.width < width,
        };
        let map = match tile.map {
            Some((start, end)) => Some(Map::decode(
                sample
                    .get(start..end)
                    .ok_or_else(|| corrupt("CRX quantization map"))?,
                tile.width.div_ceil(16),
                tile.height.div_ceil(4),
                cancel,
            )?),
            None => None,
        };
        let planes = tile
            .planes
            .par_iter()
            .map(|bands| {
                if h.levels == 0 {
                    let b = &bands[0];
                    lossless(slice(sample, b)?, pw, ph, cancel)
                } else {
                    wavelet(sample, bands, pw, ph, edges, map.as_ref(), cancel)
                }
            })
            .collect::<Vec<_>>();
        let planes = planes.into_iter().collect::<Result<Vec<_>>>()?;
        if out.is_empty() {
            out = zeros(count)?;
        }
        for (p, plane) in planes.iter().enumerate() {
            check_cancel(cancel)?;
            if plane.len() != pw * ph {
                return Err(corrupt("CRX plane size"));
            }
            let (dx, dy) = (p % 2, p / 2);
            for (j, row) in plane.chunks_exact(pw).enumerate() {
                if j % 64 == 0 {
                    check_cancel(cancel)?;
                }
                let y = tile.top + 2 * j + dy;
                let line = &mut out[y * width..(y + 1) * width];
                for (i, &v) in row.iter().enumerate() {
                    line[tile.left + 2 * i + dx] = v.saturating_add(mid).clamp(0, max) as u16;
                }
            }
        }
    }
    // A decode cancelled during the last assembly is not a success.
    check_cancel(cancel)?;
    Ok(out)
}

/// A band's coded bytes.
fn slice<'a>(sample: &'a [u8], b: &Band) -> Result<&'a [u8]> {
    sample
        .get(b.start..b.end)
        .ok_or_else(|| corrupt("CRX subband past the data"))
}

/// A Golomb-Rice code with parameter `k`: a unary quotient (zeros ended by
/// a one) and `k` low bits; 41 zeros escape a 21-bit value.
#[inline]
fn rice(p: &mut Msb, k: u32) -> Result<u32> {
    let w = p.peek(32);
    let zeros = if w != 0 {
        let z = w.leading_zeros();
        p.skip(z + 1);
        z
    } else {
        p.skip(32);
        let mut z = 32;
        loop {
            if p.overrun() {
                return Err(corrupt("CRX code past the data"));
            }
            if p.get(1) == 1 {
                break;
            }
            z += 1;
            if z > 41 {
                return Err(corrupt("CRX code too long"));
            }
        }
        z
    };
    if zeros == 41 {
        return Ok(p.get(21));
    }
    Ok((zeros << k) | p.get(k))
}

/// The value a mapped residual stands for: 0, −1, 1, −2, 2…
#[inline]
fn signed(s: u32) -> i32 {
    (s >> 1) as i32 ^ -((s & 1) as i32)
}

/// The next Rice parameter after a residual (or estimate) `s`.
#[inline]
fn adapt(k: u32, s: u32) -> u32 {
    if s >= 3 << k {
        if s >= 3 << (k + 1) {
            (k + 2).min(20)
        } else {
            (k + 1).min(20)
        }
    } else if k > 0 && s < 1 << (k - 1) {
        k - 1
    } else {
        k
    }
}

/// The median predictor of JPEG-LS: the left or upper neighbour, or their
/// gradient from the upper-left one.
#[inline]
fn median(left: i32, above: i32, corner: i32) -> i32 {
    if corner >= left.max(above) {
        left.min(above)
    } else if corner <= left.min(above) {
        left.max(above)
    } else {
        left.wrapping_add(above).wrapping_sub(corner)
    }
}

/// JPEG-LS's run-length exponents (T.87, table A.1's J).
const RUN: [u32; 32] = [
    0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 9, 10, 11, 12, 13,
    14, 15,
];

/// The length of a run (at least 1, at most `left`): each one bit adds a
/// segment of 2^J (moving to the next exponent), a zero bit ends the run with
/// J more bits (moving back one).
fn run(p: &mut Msb, index: &mut usize, left: usize) -> Result<usize> {
    let mut n = 1usize;
    while n < left {
        let j = RUN[*index];
        if p.get(1) == 0 {
            n += p.get(j) as usize;
            *index = index.saturating_sub(1);
            if n > left {
                return Err(corrupt("CRX run past its row"));
            }
            return Ok(n);
        }
        n += 1 << j;
        if n <= left {
            *index = (*index + 1).min(RUN.len() - 1);
        }
    }
    Ok(n.min(left))
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// A zeroed buffer of `n` values, allocated fallibly.
fn zeros<T: Clone + Default>(n: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n)
        .map_err(|_| Error::TooLarge("CRX buffer"))?;
    v.resize(n, T::default());
    Ok(v)
}

/// Appends a decoded row to `out`, which grows only as rows decode: a
/// stream that ends early fails before the whole plane is allocated.
fn push_row(out: &mut Vec<i32>, row: &[i32]) -> Result<()> {
    out.try_reserve(row.len())
        .map_err(|_| Error::TooLarge("CRX plane"))?;
    out.extend_from_slice(row);
    Ok(())
}

/// A losslessly coded `w × h` plane (or low-pass band).
fn lossless(src: &[u8], w: usize, h: usize, cancel: &AtomicBool) -> Result<Vec<i32>> {
    if w == 0 || h == 0 {
        return Err(corrupt("CRX empty plane"));
    }
    let mut out = Vec::new();
    let (mut prev, mut cur) = (zeros::<i32>(w)?, zeros::<i32>(w)?);
    let mut p = Msb::new(src);
    let (mut k, mut index) = (0u32, 0usize);
    for y in 0..h {
        if y % 64 == 0 {
            check_cancel(cancel)?;
        }
        let mut left = prev[0];
        let mut x = 0;
        while x < w {
            let mut from_above = false;
            if x + 1 < w && left == prev[x] && left == prev[x + 1] && p.get(1) == 1 {
                let n = run(&mut p, &mut index, w - x)?;
                cur[x..x + n].fill(left);
                x += n;
                if x == w {
                    break;
                }
                from_above = true;
            }
            let above = prev[x];
            let corner = if x == 0 { above } else { prev[x - 1] };
            let right = if x + 1 < w { prev[x + 1] } else { above };
            let s = rice(&mut p, k)?;
            let predicted = if from_above {
                above
            } else {
                median(left, above, corner)
            };
            let v = predicted.wrapping_add(signed(s));
            cur[x] = v;
            left = v;
            let estimate = if y > 0 && x + 1 < w {
                let e = (u64::from(s) + 2 * u64::from(right.abs_diff(above))) / 2;
                u32::try_from(e).unwrap_or(u32::MAX)
            } else {
                s
            };
            k = adapt(k, estimate);
            x += 1;
        }
        if p.overrun() {
            return Err(corrupt("CRX plane past its data"));
        }
        push_row(&mut out, &cur)?;
        std::mem::swap(&mut prev, &mut cur);
    }
    Ok(out)
}

/// A high-pass subband of `w × h` coefficients.
fn high_pass(src: &[u8], w: usize, h: usize, cancel: &AtomicBool) -> Result<Vec<i32>> {
    if w == 0 || h == 0 {
        return Err(corrupt("CRX empty subband"));
    }
    let mut out = Vec::new();
    let (mut prev, mut cur) = (zeros::<i32>(w)?, zeros::<i32>(w)?);
    // The Rice parameter after each coefficient of the upper and this row.
    let (mut ks_prev, mut ks) = (zeros::<u32>(w)?, zeros::<u32>(w)?);
    let mut p = Msb::new(src);
    let (mut k, mut index) = (0u32, 0usize);
    for y in 0..h {
        if y % 64 == 0 {
            check_cancel(cancel)?;
        }
        let mut left = 0;
        let mut x = 0;
        while x < w {
            let mut nonzero = false;
            if x + 1 < w && left == 0 && prev[x] == 0 && prev[x + 1] == 0 {
                if p.get(1) == 1 {
                    let n = run(&mut p, &mut index, w - x)?;
                    cur[x..x + n].fill(0);
                    ks[x..x + n].fill(0);
                    x += n;
                    if x == w {
                        break;
                    }
                }
                // After a run, or instead of one, the coefficient is not 0.
                nonzero = true;
            }
            let s = rice(&mut p, k)?;
            let v = signed(s.saturating_add(u32::from(nonzero)));
            cur[x] = v;
            left = v;
            k = adapt(k, s);
            // A larger parameter above and to the right raises it by one.
            if y > 0 && x + 1 < w && ks_prev[x + 1] > k + 1 {
                k += 1;
            }
            ks[x] = k;
            x += 1;
        }
        if p.overrun() {
            return Err(corrupt("CRX subband past its data"));
        }
        push_row(&mut out, &cur)?;
        std::mem::swap(&mut prev, &mut cur);
        std::mem::swap(&mut ks, &mut ks_prev);
    }
    Ok(out)
}

/// Which sides of a tile have a neighbouring tile.
#[derive(Clone, Copy, Debug)]
struct Edges {
    before: bool,
    after: bool,
}

/// The lengths of one level of a line of `n` samples split: the low-pass
/// band as coded, the high-pass band, and the length the next level splits.
/// Without neighbours the halves are ⌈n/2⌉ and ⌊n/2⌋. A neighbour before
/// the line adds the high-pass sample before it. A neighbour after it
/// extends the line to an odd length (both halves (n + 1)/2 long), and
/// below the coarsest level the low-pass band is coded (and split again) at
/// an odd length too: its last sample is not used (our measurements).
fn split(n: usize, edges: Edges, coarsest: bool) -> Result<(usize, usize, usize)> {
    Ok(match (edges.before, edges.after) {
        (false, false) => (n.div_ceil(2), n / 2, n.div_ceil(2)),
        (true, false) => (n.div_ceil(2), n / 2 + 1, n.div_ceil(2)),
        (false, true) => {
            let half = (n | 1).div_ceil(2);
            let low = if coarsest { half } else { half | 1 };
            (low, half, low)
        }
        (true, true) => {
            return Err(Error::Unsupported(
                "C-RAW tiles between two neighbours".into(),
            ));
        }
    })
}

/// A version 2 tile's quantization parameters, one per 16 × 4 sensor pixels.
struct Map {
    width: usize,
    height: usize,
    values: Vec<i32>,
}

impl Map {
    /// A `w × h` map: its first value a 32-bit number, every later one
    /// median-predicted with a Rice-coded residual (23 zeros escape an
    /// 8-bit residual).
    fn decode(src: &[u8], w: usize, h: usize, cancel: &AtomicBool) -> Result<Map> {
        check_cancel(cancel)?;
        if w == 0 || h == 0 || w.saturating_mul(h) > crate::io::MAX_SAMPLES as usize / 16 {
            return Err(corrupt("CRX quantization map size"));
        }
        let mut values = zeros::<i32>(w * h)?;
        let first = zeros::<i32>(w)?;
        let mut p = Msb::new(src);
        let mut k = 0u32;
        for y in 0..h {
            if y % 64 == 0 {
                check_cancel(cancel)?;
            }
            let (done, rest) = values.split_at_mut(y * w);
            let prev: &[i32] = if y == 0 { &first } else { &done[(y - 1) * w..] };
            let cur = &mut rest[..w];
            let mut left = prev[0];
            for x in 0..w {
                let above = prev[x];
                let corner = if x == 0 { above } else { prev[x - 1] };
                let right = if x + 1 < w { prev[x + 1] } else { above };
                let s = if x == 0 && y == 0 {
                    p.get(32)
                } else {
                    let w = p.peek(32);
                    let zeros = w.leading_zeros().min(24);
                    if zeros >= 23 {
                        // 23 zeros and a one, then the residual.
                        if (w >> 8) != 1 {
                            return Err(corrupt("CRX quantization map code"));
                        }
                        p.skip(24);
                        p.get(8)
                    } else {
                        p.skip(zeros + 1);
                        (zeros << k) | p.get(k)
                    }
                };
                let v = median(left, above, corner).wrapping_add(signed(s));
                cur[x] = v;
                left = v;
                let estimate = if y > 0 && x + 1 < w {
                    ((u64::from(s) + 2 * u64::from(right.abs_diff(above))) / 2)
                        .min(u64::from(u32::MAX)) as u32
                } else {
                    s
                };
                k = adapt(k, estimate);
            }
            if p.overrun() {
                return Err(corrupt("CRX quantization map past its data"));
            }
        }
        Ok(Map {
            width: w,
            height: h,
            values,
        })
    }
    /// The parameter at map row `y`, column `x`.
    fn at(&self, y: usize, x: usize) -> Result<i32> {
        if y < self.height && x < self.width {
            Ok(self.values[y * self.width + x])
        } else {
            Err(corrupt("CRX quantization map too small"))
        }
    }
}

/// The quantization step of parameter `q`: H.265's level scale doubled every
/// six steps, in 64ths.
fn step(q: u32) -> i32 {
    const SCALE: [i32; 6] = [40, 45, 51, 57, 64, 72];
    if q / 6 > 24 {
        return i32::MAX;
    }
    (SCALE[(q % 6) as usize] << (q / 6)) / 64
}

/// A C-RAW plane of `w × h` samples: its subbands decoded, dequantized and
/// recombined.
fn wavelet(
    sample: &[u8],
    bands: &[Band],
    w: usize,
    h: usize,
    edges: Edges,
    map: Option<&Map>,
    cancel: &AtomicBool,
) -> Result<Vec<i32>> {
    // Per level, finest first: the line lengths split, and the coded low and
    // high-pass widths and heights.
    let mut levels = Vec::with_capacity(3);
    let (mut n, mut m) = (w, h);
    for level in 0..3 {
        let (low, high, next) = split(n, edges, level == 2)?;
        let rows = (m.div_ceil(2), m / 2);
        levels.push(((n, m), (low, high), rows));
        n = next;
        m = rows.0;
    }
    if n == 0 || m == 0 {
        return Err(corrupt("CRX plane too small"));
    }
    // Dequantizes band `i` of `bw` columns.
    let quant = |i: usize, bw: usize, v: &mut [i32]| -> Result<()> {
        match bands[i].quant {
            Quant::Fixed(q) => scale(v, step(q).max(1), cancel)?,
            Quant::Adaptive { base, gain: 0 } => {
                scale(v, i32::try_from(base).unwrap_or(i32::MAX).max(1), cancel)?;
            }
            Quant::Adaptive { base, gain } => {
                let map = map.ok_or_else(|| corrupt("CRX quantization map missing"))?;
                if bw == 0 {
                    return Ok(());
                }
                for (j, row) in v.chunks_mut(bw).enumerate() {
                    if j % 64 == 0 {
                        check_cancel(cancel)?;
                    }
                    for (x, c) in row.iter_mut().enumerate() {
                        // Level 2 bands average two map rows; level 1 bands
                        // read one.
                        let qp = if i < 7 {
                            let r = j * 2;
                            let a = map.at(r, x / 2)?;
                            let b = map.at((r + 1).min(map.height - 1), x / 2)?;
                            ((i64::from(a) + i64::from(b)) / 2) as i32
                        } else {
                            map.at(j, x / 4)?
                        };
                        let q = u32::try_from(i64::from(qp) - 124)
                            .map_err(|_| corrupt("CRX quantization parameter"))?;
                        let s = i64::from(base) + i64::from(gain) * i64::from(step(q)) / 8;
                        *c = c.saturating_mul(s.clamp(1, i64::from(i32::MAX)) as i32);
                    }
                }
            }
        }
        Ok(())
    };
    let mut ll = lossless(slice(sample, &bands[0])?, n, m, cancel)?;
    quant(0, n, &mut ll)?;
    for level in (0..3).rev() {
        let ((ow, oh), (low, high), (top, bottom)) = levels[level];
        let i = 1 + 3 * (2 - level);
        let mut hl = high_pass(slice(sample, &bands[i])?, high, top, cancel)?;
        let mut lh = high_pass(slice(sample, &bands[i + 1])?, low, bottom, cancel)?;
        let mut hh = high_pass(slice(sample, &bands[i + 2])?, high, bottom, cancel)?;
        quant(i, high, &mut hl)?;
        quant(i + 1, low, &mut lh)?;
        quant(i + 2, high, &mut hh)?;
        let parts = [
            (&ll[..], low),
            (&hl[..], high),
            (&lh[..], low),
            (&hh[..], high),
        ];
        ll = synthesize(parts, (top, bottom), ow, oh, edges, cancel)?;
    }
    Ok(ll)
}

/// Multiplies every coefficient by `step`, checking `cancel` as it goes.
fn scale(v: &mut [i32], step: i32, cancel: &AtomicBool) -> Result<()> {
    for chunk in v.chunks_mut(1 << 16) {
        check_cancel(cancel)?;
        chunk.iter_mut().for_each(|c| *c = c.saturating_mul(step));
    }
    Ok(())
}

/// One level of the inverse 5/3 wavelet, rows first, then columns: the
/// low-pass band, then the horizontally, vertically and diagonally
/// high-pass ones, each with its width; `rows` the low and high-pass heights.
fn synthesize(
    parts: [(&[i32], usize); 4],
    rows: (usize, usize),
    w: usize,
    h: usize,
    edges: Edges,
    cancel: &AtomicBool,
) -> Result<Vec<i32>> {
    let (top, bottom) = rows;
    if parts
        .iter()
        .zip([top, top, bottom, bottom])
        .any(|(&(band, bw), bh)| bw == 0 || band.len() != bw * bh)
        || top + bottom != h
    {
        return Err(corrupt("CRX subband sizes"));
    }
    let mut tmp = zeros::<i32>(w * h)?;
    for (y, out) in tmp.chunks_exact_mut(w).enumerate() {
        if y % 256 == 0 {
            check_cancel(cancel)?;
        }
        let ((lo, lw), (hi, hw), y) = if y < top {
            (parts[0], parts[1], y)
        } else {
            (parts[2], parts[3], y - top)
        };
        line(
            &lo[y * lw..(y + 1) * lw],
            &hi[y * hw..(y + 1) * hw],
            out,
            edges.before,
            edges.after,
        );
    }
    let mut out = zeros::<i32>(w * h)?;
    let (mut lo, mut hi, mut col) = (zeros(top)?, zeros(bottom)?, zeros(h)?);
    for x in 0..w {
        if x % 256 == 0 {
            check_cancel(cancel)?;
        }
        for (y, v) in lo.iter_mut().enumerate() {
            *v = tmp[y * w + x];
        }
        for (y, v) in hi.iter_mut().enumerate() {
            *v = tmp[(y + top) * w + x];
        }
        line(&lo, &hi, &mut col, false, false);
        for (y, &v) in col.iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    Ok(out)
}

/// The inverse 5/3 lifting of one line from its low and high-pass halves;
/// with a neighbour before it the high-pass half starts one sample early,
/// and with one after it both halves run one sample on. Without a neighbour
/// the line is extended symmetrically.
fn line(low: &[i32], high: &[i32], out: &mut [i32], before: bool, after: bool) {
    let n = out.len();
    if high.is_empty() {
        if let (Some(o), Some(&l)) = (out.first_mut(), low.first()) {
            *o = l;
        }
        return;
    }
    let b = usize::from(before);
    let hi = |i: usize| high[i.min(high.len() - 1)];
    let even = |i: usize| {
        let l = hi((i + b).saturating_sub(1));
        let r = hi(i + b);
        low[i.min(low.len() - 1)].wrapping_sub((l.wrapping_add(r).wrapping_add(2)) >> 2)
    };
    for i in 0..n.div_ceil(2) {
        out[2 * i] = even(i);
    }
    for i in 0..n / 2 {
        let l = out[2 * i];
        let r = if 2 * i + 2 < n {
            out[2 * i + 2]
        } else if after {
            even(i + 1)
        } else {
            l
        };
        out[2 * i + 1] = hi(i + b).wrapping_add(l.wrapping_add(r) >> 1);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn no() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// A tile, plane or subband marker of the version 1 coding: its size and
    /// flags.
    fn marker(out: &mut Vec<u8>, code: u16, size: u32, flags: u32) {
        out.extend(code.to_be_bytes());
        out.extend(8u16.to_be_bytes());
        out.extend(size.to_be_bytes());
        out.extend(flags.to_be_bytes());
    }

    pub(crate) fn header(w: u32, h: u32, levels: u8, header_size: u32) -> Header {
        Header {
            version: 0x100,
            width: w,
            height: h,
            tile_width: w,
            tile_height: h,
            bits: 14,
            planes: 4,
            cfa: 0,
            encoding: 0,
            levels,
            header_size,
        }
    }

    /// One tile of four lossless 2 × 2 planes of value 1. Worked by hand:
    /// row 0 is a declined run (0), residual 2 → 1 (`001`, k 0), then 0
    /// predicted from the left (`1`); row 1 runs the whole row (`1`, then
    /// one segment of 2⁰ (`1`)). `0001111` padded is 0x1E.
    pub(crate) fn flat() -> Vec<u8> {
        let mut out = Vec::new();
        marker(&mut out, 0xff01, 4, 0);
        for p in 0..4 {
            marker(&mut out, 0xff02, 1, p << 28);
            marker(&mut out, 0xff03, 1, 0);
        }
        out.extend([0x1e; 4]);
        out
    }

    #[test]
    fn residuals_map_to_alternating_signs() {
        assert_eq!([0, 1, 2, 3, 4].map(signed), [0, -1, 1, -2, 2]);
        assert_eq!(median(-7, -8, -9), -7);
        assert_eq!(median(-7, -8, -6), -8);
        assert_eq!(median(10, 20, 14), 16);
        // Up by two past 3·2^(k+1), down by one under 2^(k−1).
        assert_eq!(
            [
                adapt(0, 2),
                adapt(0, 3),
                adapt(0, 6),
                adapt(3, 3),
                adapt(3, 4)
            ],
            [0, 1, 2, 2, 3]
        );
        assert_eq!(adapt(20, 1 << 30), 20);
    }

    #[test]
    fn rice_codes_and_their_escape() {
        // 2 zeros, a one, then k = 2 bits 10: (2 << 2) | 2.
        let mut p = Msb::new(&[0b0011_0000]);
        assert_eq!(rice(&mut p, 2).unwrap(), 10);
        // 41 zeros, a one, a 21-bit value.
        let mut bits = vec![0u8; 5];
        bits.push(0b0100_0000 | 0x20);
        bits.extend([0, 0x02]);
        let mut p = Msb::new(&bits);
        assert_eq!(rice(&mut p, 0).unwrap(), (1 << 20) | 1);
        assert!(rice(&mut Msb::new(&[0; 8]), 0).is_err());
    }

    #[test]
    fn runs_grow_and_shrink_their_segments() {
        // Index 0: three segments of 1 fill 4; the index moves to 3.
        let mut i = 0;
        assert_eq!(run(&mut Msb::new(&[0b1110_0000]), &mut i, 4).unwrap(), 4);
        assert_eq!(i, 3);
        // A zero ends a run with J bits: index 4 (J 1), segment `1` (2
        // more), stop `0`, one bit `1`: 1 + 2 + 1 = 4 of 9; index back to 4.
        let mut i = 4;
        assert_eq!(run(&mut Msb::new(&[0b1010_0000]), &mut i, 9).unwrap(), 4);
        assert_eq!(i, 4);
        assert!(run(&mut Msb::new(&[0b0111_1111]), &mut 20, 3).is_err());
    }

    #[test]
    fn decodes_lossless_planes_into_the_mosaic() {
        let s = flat();
        let out = decode(&header(4, 4, 0, 108), &s, &no()).unwrap();
        assert_eq!(out, vec![8193; 16]);
    }

    #[test]
    fn truncated_or_damaged_data_is_an_error_not_a_panic() {
        let s = flat();
        let h = header(4, 4, 0, 108);
        for n in 0..s.len() {
            assert!(decode(&h, &s[..n], &no()).is_err(), "{n}");
        }
        for i in 0..s.len() {
            for v in [0, 1, 0x7f, 0xff] {
                let mut d = s.clone();
                d[i] = v;
                let _ = decode(&h, &d, &no());
            }
        }
    }

    #[test]
    fn hostile_headers_and_cancellation() {
        let s = flat();
        let mut h = header(4, 4, 0, 108);
        h.width = 70_000;
        assert!(matches!(decode(&h, &s, &no()), Err(Error::TooLarge(_))));
        let mut h = header(4, 4, 0, 108);
        h.levels = 2;
        assert!(matches!(decode(&h, &s, &no()), Err(Error::Unsupported(_))));
        let mut h = header(4, 4, 0, 108);
        h.tile_width = 0;
        assert!(decode(&h, &s, &no()).is_err());
        let mut h = header(4, 4, 0, u32::MAX);
        h.tile_width = 2;
        assert!(decode(&h, &s, &no()).is_err());
        // A large image the data cannot hold fails before it is allocated.
        let h = header(60_000, 26_000, 0, 108);
        assert!(decode(&h, &s, &no()).is_err());
        let cancel = AtomicBool::new(true);
        assert_eq!(
            decode(&header(4, 4, 0, 108), &s, &cancel),
            Err(Error::Cancelled)
        );
    }

    /// A 16 × 16 C-RAW tile, three levels: per plane a 1 × 1 low-pass band
    /// of `ll` (coded with residual `code`, k 0) and quantization parameter
    /// `q`, zero high-pass bands. Each zero band of 1 × 1 is `1`; of 2 × 2
    /// two whole-row runs (`11`, `11`); of 4 × 4 runs `1111`, `111`, `111`,
    /// `111` (segments 2⁰ × 3, then 2⁰ and 2¹ at indices 3 and 4, then 2¹
    /// twice from index 5 and 6, the last overrunning the row): 13 ones.
    pub(crate) fn craw(code: u8, q: u32) -> Vec<u8> {
        let mut s = Vec::new();
        marker(&mut s, 0xff01, 4 * 13, 0);
        for p in 0..4 {
            marker(&mut s, 0xff02, 13, p << 28);
            for b in 0..10u32 {
                marker(
                    &mut s,
                    0xff03,
                    if b >= 7 { 2 } else { 1 },
                    (b << 28) | (q << 19),
                );
            }
        }
        for _ in 0..4 {
            s.push(code);
            s.extend([0x80; 3]);
            s.extend([0xf0; 3]);
            for _ in 0..3 {
                s.extend([0xff, 0xf8]);
            }
        }
        s
    }

    #[test]
    fn decodes_three_wavelet_levels() {
        let h = header(16, 16, 3, 12 + 4 * 12 * 11);
        // Low-pass 0: every sample is the midpoint.
        assert_eq!(decode(&h, &craw(0x80, 4), &no()).unwrap(), vec![8192; 256]);
        // Low-pass 3 (residual 6: six zeros and a one), parameter 10: step
        // (64 · 2) / 64 = 2, so 6 throughout.
        assert_eq!(decode(&h, &craw(0x02, 10), &no()).unwrap(), vec![8198; 256]);
        let s = craw(0x02, 10);
        for n in 0..s.len() {
            assert!(decode(&h, &s[..n], &no()).is_err(), "{n}");
        }
    }

    /// `craw`'s tile in version 2 of the coding: 16-byte tile and subband
    /// headers, a 1 × 4 quantization map (QP 150 throughout: the literal 300,
    /// then residuals 0 at k 2, 1 and 0, `100` `10` `1`), the low-pass band
    /// scaled by `base`, the finer high-pass bands adaptively.
    fn craw2(base: u32) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend(0xff11u16.to_be_bytes());
        s.extend(16u16.to_be_bytes());
        s.extend((8 + 4 * 13u32).to_be_bytes());
        s.extend([0; 4]);
        s.extend(5u32.to_be_bytes());
        s.extend(3u16.to_be_bytes());
        s.extend([0; 2]);
        for p in 0..4 {
            marker(&mut s, 0xff12, 13, p << 28);
            for b in 0..10u32 {
                s.extend(0xff13u16.to_be_bytes());
                s.extend(16u16.to_be_bytes());
                s.extend(if b >= 7 { 2u32 } else { 1 }.to_be_bytes());
                s.extend(((b << 28) | if b >= 4 { 8 } else { 0 }).to_be_bytes());
                s.extend(if b == 0 { base } else { 1 }.to_be_bytes());
                s.extend([0; 4]);
            }
        }
        s.extend([0, 0, 1, 0x2c, 0x94, 0, 0, 0]);
        for _ in 0..4 {
            s.push(0x02);
            s.extend([0x80; 3]);
            s.extend([0xf0; 3]);
            for _ in 0..3 {
                s.extend([0xff, 0xf8]);
            }
        }
        s
    }

    /// Bits written most significant first, for the encoders below.
    #[derive(Default)]
    struct Bits {
        out: Vec<u8>,
        n: usize,
    }

    impl Bits {
        fn put(&mut self, v: u32, len: u32) {
            for i in (0..len).rev() {
                if self.n.is_multiple_of(8) {
                    self.out.push(0);
                }
                if (v >> i) & 1 == 1 {
                    *self.out.last_mut().unwrap() |= 0x80 >> (self.n % 8);
                }
                self.n += 1;
            }
        }
    }

    /// A `w × h` plane (or band) of zeros: every row a run (the run mode's
    /// segments as `run` reads them), a one-sample row a residual of 0.
    fn zeros_coded(w: usize, h: usize) -> Vec<u8> {
        let mut b = Bits::default();
        let mut index = 0usize;
        for _ in 0..h {
            if w == 1 {
                b.put(1, 1);
                continue;
            }
            b.put(1, 1);
            let mut n = 1;
            while n < w {
                let j = RUN[index];
                if n + (1 << j) <= w {
                    b.put(1, 1);
                    n += 1 << j;
                    index = (index + 1).min(RUN.len() - 1);
                } else {
                    b.put(0, 1);
                    b.put((w - n) as u32, j);
                    n = w;
                    index = index.saturating_sub(1);
                }
            }
        }
        b.out
    }

    /// A `w × h` quantization map of QP 150: the literal 300, then residuals
    /// of 0 with the parameter as `adapt` moves it.
    fn map_coded(w: usize, h: usize) -> Vec<u8> {
        let mut b = Bits::default();
        b.put(300, 32);
        let mut k = adapt(0, 300);
        for _ in 1..w * h {
            b.put(1, 1);
            b.put(0, k);
            k = adapt(k, 0);
        }
        b.out
    }

    /// A version 2 C-RAW tile of `w × h` sensor pixels: zero planes, the
    /// map of QP 150, the finer bands adaptive.
    fn craw2_large(w: usize, h: usize) -> (Header, Vec<u8>) {
        let (pw, ph) = (w / 2, h / 2);
        let none = Edges {
            before: false,
            after: false,
        };
        // Band sizes, coarsest first: the low-pass band, then per level the
        // horizontal, vertical and diagonal high-pass bands.
        let mut levels = Vec::new();
        let (mut n, mut m) = (pw, ph);
        for level in 0..3 {
            let (low, high, next) = split(n, none, level == 2).unwrap();
            levels.push((low, high, m.div_ceil(2), m / 2));
            n = next;
            m = m.div_ceil(2);
        }
        let mut sizes = vec![(n, m)];
        for &(low, high, top, bottom) in levels.iter().rev() {
            sizes.extend([(high, top), (low, bottom), (high, bottom)]);
        }
        let bands: Vec<Vec<u8>> = sizes.iter().map(|&(bw, bh)| zeros_coded(bw, bh)).collect();
        let map = map_coded(w.div_ceil(16), h.div_ceil(4));
        let pad = (8 - map.len() % 8) % 8;
        let plane: usize = bands.iter().map(Vec::len).sum();
        let mut s = Vec::new();
        s.extend(0xff11u16.to_be_bytes());
        s.extend(16u16.to_be_bytes());
        s.extend(((map.len() + pad + 4 * plane) as u32).to_be_bytes());
        s.extend([0; 4]);
        s.extend((map.len() as u32).to_be_bytes());
        s.extend((pad as u16).to_be_bytes());
        s.extend([0; 2]);
        for p in 0..4 {
            marker(&mut s, 0xff12, plane as u32, p << 28);
            for (b, band) in bands.iter().enumerate() {
                s.extend(0xff13u16.to_be_bytes());
                s.extend(16u16.to_be_bytes());
                s.extend((band.len() as u32).to_be_bytes());
                s.extend((((b as u32) << 28) | if b >= 4 { 8 } else { 0 }).to_be_bytes());
                s.extend(1u32.to_be_bytes());
                s.extend([0; 4]);
            }
        }
        let header_size = s.len() as u32;
        s.extend(&map);
        s.extend(vec![0; pad]);
        for _ in 0..4 {
            for band in &bands {
                s.extend(band);
            }
        }
        let mut h = header(w as u32, h as u32, 3, header_size);
        h.version = 0x200;
        (h, s)
    }

    #[test]
    fn a_large_version_2_tile_decodes_and_cancels_midway() {
        let (h, s) = craw2_large(64, 32);
        assert_eq!(decode(&h, &s, &no()).unwrap(), vec![8192; 64 * 32]);
        let (h, s) = craw2_large(4096, 2048);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let setter = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                cancel.store(true, Ordering::Relaxed);
                std::time::Instant::now()
            })
        };
        let result = decode(&h, &s, &cancel);
        let returned = std::time::Instant::now();
        let set = setter.join().unwrap();
        assert_eq!(result, Err(Error::Cancelled));
        assert!(returned.duration_since(set) < std::time::Duration::from_millis(250));
    }

    #[test]
    fn maps_scaling_and_assembly_check_cancellation() {
        let yes = AtomicBool::new(true);
        let map = map_coded(64, 64);
        assert!(Map::decode(&map, 64, 64, &no()).is_ok());
        assert_eq!(
            Map::decode(&map, 64, 64, &yes).err(),
            Some(Error::Cancelled)
        );
        let mut v = vec![1; 1 << 18];
        assert_eq!(scale(&mut v, 2, &yes), Err(Error::Cancelled));
        // Cancelled once every plane is decoded: still not a success.
        let s = flat();
        let cancel = AtomicBool::new(false);
        let h = header(4, 4, 0, 108);
        assert!(decode(&h, &s, &cancel).is_ok());
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(decode(&h, &s, &cancel), Err(Error::Cancelled));
    }

    #[test]
    fn decodes_version_2_with_quantization_maps() {
        let mut h = header(16, 16, 3, 20 + 4 * (12 + 10 * 20));
        h.version = 0x200;
        // Low-pass 3 scaled by 2.
        assert_eq!(decode(&h, &craw2(2), &no()).unwrap(), vec![8198; 256]);
        let s = craw2(2);
        for n in 0..s.len() {
            assert!(decode(&h, &s[..n], &no()).is_err(), "{n}");
        }
        for i in 0..s.len() {
            for v in [0, 0x7f, 0xff] {
                let mut d = s.clone();
                d[i] = v;
                let _ = decode(&h, &d, &no());
            }
        }
        // Tiles stacked vertically are not decoded.
        let mut v = header(16, 16, 3, 540);
        v.tile_height = 8;
        assert!(matches!(
            decode(&v, &craw(0x02, 10), &no()),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn quantization_steps() {
        assert_eq!(
            [4, 10, 16, 22, 26, 32, 36, 41].map(step),
            [1, 2, 4, 8, 12, 25, 40, 72]
        );
        assert_eq!(step(200), i32::MAX);
    }

    /// The forward integer 5/3 transform of ITU T.800 (F.4.8.2), with
    /// symmetric extension, written from the standard's equations.
    fn forward(x: &[i32]) -> (Vec<i32>, Vec<i32>) {
        let n = x.len();
        let at = |i: isize| -> i32 {
            let i = if i < 0 { -i } else { i } as usize;
            if i >= n { x[2 * (n - 1) - i] } else { x[i] }
        };
        let d: Vec<i32> = (0..n / 2)
            .map(|k| {
                let i = 2 * k as isize + 1;
                at(i) - (at(i - 1) + at(i + 1)).div_euclid(2)
            })
            .collect();
        let dat = |k: isize| -> i32 {
            // The high-pass samples extended as the signal is.
            let k = if k < 0 { -k - 1 } else { k } as usize;
            d[k.min(d.len() - 1)]
        };
        let s = (0..n.div_ceil(2))
            .map(|k| x[2 * k] + (dat(k as isize - 1) + dat(k as isize) + 2).div_euclid(4))
            .collect();
        (s, d)
    }

    #[test]
    fn the_inverse_lifting_undoes_the_forward_transform() {
        for n in 2..24 {
            let x: Vec<i32> = (0..n)
                .map(|i| ((i * 37 + i * i * 11) % 97) as i32 - 48)
                .collect();
            let (s, d) = forward(&x);
            let mut out = vec![0; n];
            line(&s, &d, &mut out, false, false);
            assert_eq!(out, x, "{n}");
        }
    }

    #[test]
    fn line_lengths_at_tile_edges() {
        let none = Edges {
            before: false,
            after: false,
        };
        let before = Edges {
            before: true,
            after: false,
        };
        let after = Edges {
            before: false,
            after: true,
        };
        assert_eq!(split(1572, none, false).unwrap(), (786, 786, 786));
        assert_eq!(split(1572, before, false).unwrap(), (786, 787, 786));
        // Measured on EOS M50 files: 1572 → 787, 787 → 395 (coded; 394
        // used), the coarsest 395 → 198 and 198.
        assert_eq!(split(1572, after, false).unwrap(), (787, 787, 787));
        assert_eq!(split(787, after, false).unwrap(), (395, 394, 395));
        assert_eq!(split(395, after, true).unwrap(), (198, 198, 198));
        assert!(
            split(
                10,
                Edges {
                    before: true,
                    after: true
                },
                false
            )
            .is_err()
        );
    }

    #[test]
    fn quantization_maps() {
        // 1 × 2: the first value a 32-bit literal 300 (QP 150), the second
        // predicted from it with residual 0 (`1`).
        let m = Map::decode(&[0, 0, 1, 0x2c, 0x80], 1, 2, &no()).unwrap();
        assert_eq!(m.values, [150, 150]);
        assert!(Map::decode(&[0, 0, 1], 1, 2, &no()).is_err());
        assert!(m.at(2, 0).is_err());
    }
}
