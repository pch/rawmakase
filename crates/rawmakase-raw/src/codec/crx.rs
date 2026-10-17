//! Canon's CRX coding of CR3 sensor data, from bytes to samples.
//!
//! The image header (the CR3 track's `CMP1` box) gives the image and tile
//! sizes, the bit depth, the number of colour planes (four: the Bayer cell's
//! sites), the number of wavelet levels (0: lossless) and the size of the
//! tile headers at the start of the coded data. Those headers are a sequence
//! of markers, big-endian: per tile (`FF01`) its size, per plane (`FF02`)
//! its size and flags, and per subband (`FF03`, one for a lossless plane) its
//! size and flags; the coded data follows them in the same order.
//!
//! Each plane of a tile is a half-width, half-height image coded on its own,
//! predicted from the median of the left, upper and upper-left neighbours
//! (as JPEG-LS, ITU T.87, predicts) and the residuals Golomb-Rice coded with
//! a parameter adapted to the previous residual and the upper row's
//! gradient. Where the left and the two upper neighbours agree, a flag
//! starts a run of the left value whose length is coded as in JPEG-LS's run
//! mode; a run ending inside the row is followed by one residual predicted
//! from above.
//!
//! A sample is the plane's value plus half the bit depth's range.
//!
//! Sources: Laurent Clévy's public description of the CR3 format (the
//! `CMP1` fields and the marker layout), ITU T.87 (median prediction, the
//! run-mode index table), and our own measurements.
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
        if self.version != 0x100
            || self.planes != 4
            || self.encoding != 0
            || self.levels != 0
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

#[derive(Clone, Debug)]
struct Band {
    /// Where the band's coded data is in the sample.
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct Tile {
    left: usize,
    top: usize,
    width: usize,
    height: usize,
    /// Each plane's subbands.
    planes: Vec<Vec<Band>>,
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
        let (body, len) = marker(0xff01)?;
        if len < 8 {
            return Err(corrupt("CRX tile header"));
        }
        let size = be(head, body, 4)? as usize;
        let tile_end = data
            .checked_add(size)
            .filter(|&e| e <= sample.len())
            .ok_or_else(|| corrupt("CRX tile past the data"))?;
        let mut planes = Vec::with_capacity(4);
        for _ in 0..h.planes {
            let (body, _) = marker(0xff02)?;
            let size = be(head, body, 4)? as usize;
            let plane_end = data
                .checked_add(size)
                .filter(|&e| e <= tile_end)
                .ok_or_else(|| corrupt("CRX plane past its tile"))?;
            let mut list = Vec::with_capacity(bands);
            for _ in 0..bands {
                let (body, _) = marker(0xff03)?;
                let size = be(head, body, 4)? as usize;
                let end = data
                    .checked_add(size)
                    .filter(|&e| e <= plane_end)
                    .ok_or_else(|| corrupt("CRX subband past its plane"))?;
                list.push(Band { start: data, end });
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
        });
    }
    Ok(tiles)
}

/// Decodes the sensor data in `sample`, which the image header `h` describes:
/// `width × height` samples.
pub(crate) fn decode(h: &Header, sample: &[u8], cancel: &AtomicBool) -> Result<Vec<u16>> {
    h.check()?;
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
        let planes = tile
            .planes
            .par_iter()
            .map(|bands| lossless(slice(sample, &bands[0])?, pw, ph, cancel))
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
        h.levels = 3;
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
}
