//! Nikon's High Efficiency and High Efficiency★ raws: a JPEG XS codestream (ISO/IEC
//! 21122, as intoPIX's TicoRAW writes it) that LibRaw cannot decode.
//!
//! The codestream is standard JPEG XS with four components on a quarter-size grid,
//! one per 2×2 Bayer quad position, and these departures from it:
//!
//! - The picture header carries 13 vendor bytes after the standard fields, among them
//!   the parameters of the transfer curve that maps decoded values to raw values.
//! - The component table has a third byte per component, its own wavelet
//!   decomposition; the third component (the difference between the two greens) has
//!   none.
//! - Packet headers always take the long form.
//! - The components are a star-tetrix decorrelation of the quad (the green at the red
//!   row's site, red, the other green, blue), though the header declares no colour
//!   transform and has no parameters for one.
//! - Coded values are a square-root companding of the raw values, undone by the
//!   transfer curve rather than by the standard linear output scaling.
//!
//! The decorrelation's lifting steps and the transfer curve, rounding included, are
//! the ones Adobe DNG Converter reproduces bit for bit; the tests compare against its
//! output.
use crate::error::{Error, Result, corrupt};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Fails with a `Corrupt` error saying `format!(…)` unless `cond` holds.
macro_rules! ensure {
    ($cond:expr, $($what:tt)+) => {
        if !$cond {
            return Err(corrupt(format!($($what)+)));
        }
    };
}

/// A missing value as a `Corrupt` error saying `what`.
trait Context<T> {
    fn context(self, what: &str) -> Result<T>;
}
impl<T> Context<T> for Option<T> {
    fn context(self, what: &str) -> Result<T> {
        self.ok_or_else(|| corrupt(what))
    }
}
impl<T, E> Context<T> for std::result::Result<T, E> {
    fn context(self, what: &str) -> Result<T> {
        self.map_err(|_| corrupt(what))
    }
}

/// The JPEG XS markers Nikon writes.
const SOC: u16 = 0xff10;
const EOC: u16 = 0xff11;
const PIH: u16 = 0xff12;
const CDT: u16 = 0xff13;
const WGT: u16 = 0xff14;
const SLH: u16 = 0xff20;
const CAP: u16 = 0xff50;

/// Decodes a codestream into `out`, `width` by `height` raw values `pitch` apart,
/// which must be the codestream's own size. Values are 14-bit, black included.
pub fn decode(
    data: &[u8],
    out: &mut [u16],
    width: usize,
    height: usize,
    pitch: usize,
) -> Result<()> {
    decode_cancellable(data, out, width, height, pitch, &AtomicBool::new(false))
}

/// [`decode`], stopping with `Error::Cancelled` soon after `cancel` is set:
/// it is checked before each precinct and between the stages.
pub(crate) fn decode_cancellable(
    data: &[u8],
    out: &mut [u16],
    width: usize,
    height: usize,
    pitch: usize,
    cancel: &AtomicBool,
) -> Result<()> {
    let cancelled = || {
        if cancel.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    cancelled()?;
    let header = Header::parse(data)?;
    ensure!(
        (header.width, header.height) == (width, height),
        "High Efficiency codestream is {}×{}, the raw {}×{}",
        header.width,
        header.height,
        width,
        height
    );
    ensure!(
        pitch >= width && out.len() >= pitch * height,
        "raw buffer too small"
    );
    let layout = Layout::new(&header)?;
    let precincts = header.precincts(data)?;
    let mut coefficients = Coefficients::new(&layout);
    decode_slices(
        data,
        &header,
        &layout,
        &precincts,
        &mut coefficients,
        cancel,
    )?;
    cancelled()?;
    let mut planes = coefficients.inverse_wavelet(&layout);
    cancelled()?;
    inverse_star_tetrix(&mut planes, layout.width, layout.height);
    cancelled()?;
    let transfer = header.transfer_curve();
    let [g1, red, g2, blue] = &planes;
    let w = layout.width;
    out.par_chunks_mut(2 * pitch)
        .take(layout.height)
        .enumerate()
        .for_each(|(y, rows)| {
            let (top, bottom) = rows.split_at_mut(pitch);
            let plane = y * w..(y + 1) * w;
            let (r, g, h, b) = (
                &red[plane.clone()],
                &g1[plane.clone()],
                &g2[plane.clone()],
                &blue[plane],
            );
            for x in 0..w {
                top[2 * x] = transfer.apply(r[x]);
                top[2 * x + 1] = transfer.apply(g[x]);
                bottom[2 * x] = transfer.apply(h[x]);
                bottom[2 * x + 1] = transfer.apply(b[x]);
            }
        });
    Ok(())
}

/// The picture header, component table and weights, as far as decoding needs them.
#[derive(Debug)]
struct Header {
    /// The full raw size; each component is half of it both ways.
    width: usize,
    height: usize,
    /// Precincts per slice.
    slice_height: usize,
    /// Coefficients per code group, code groups per significance group.
    group: usize,
    significance_group: usize,
    /// Fractional bits of the wavelet coefficients, bits of a raw-coded GCLI.
    fraction_bits: u32,
    raw_gcli_bits: u32,
    /// Per component: horizontal decomposition levels, 0 for none (the one
    /// component without a wavelet).
    levels: Vec<u32>,
    /// (gain, priority) per band, in the order the packets first carry them.
    weights: Vec<(u8, u8)>,
    /// The vendor bytes' transfer parameters: black, gain (Q16) and knee.
    black: i64,
    gain: i64,
    knee: i64,
    /// Offset of the first slice header.
    first_slice: usize,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}
impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let bytes = self
            .data
            .get(self.at..self.at + n)
            .context("High Efficiency codestream is truncated")?;
        self.at += n;
        Ok(bytes)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
}

impl Header {
    fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader { data, at: 0 };
        ensure!(r.u16()? == SOC, "not a JPEG XS codestream");
        let mut header: Option<Header> = None;
        let mut components = 0;
        loop {
            let at = r.at;
            let marker = r.u16()?;
            if marker == SLH {
                let mut h = header.context("JPEG XS codestream without a picture header")?;
                ensure!(
                    h.levels.len() == components,
                    "JPEG XS codestream without a component table"
                );
                h.first_slice = at;
                return Ok(h);
            }
            ensure!(marker != EOC, "JPEG XS codestream without slices");
            let length = usize::from(r.u16()?);
            ensure!(length >= 2, "bad JPEG XS marker length");
            let body = r.bytes(length - 2)?;
            let mut b = Reader { data: body, at: 0 };
            match marker {
                CAP => {}
                PIH => {
                    ensure!(body.len() == 37, "unknown High Efficiency picture header");
                    let _codestream_length = b.bytes(4)?;
                    let _profile_level = b.bytes(4)?;
                    let width = usize::from(b.u16()?);
                    let height = usize::from(b.u16()?);
                    let precinct_width = b.u16()?;
                    let slice_height = usize::from(b.u16()?);
                    components = usize::from(b.u8()?);
                    let group = usize::from(b.u8()?);
                    let significance_group = usize::from(b.u8()?);
                    let _nominal_bits = b.u8()?;
                    let fq_br = b.u8()?;
                    let order = b.u8()?;
                    let decomposition = b.u8()?;
                    let flags = b.u8()?;
                    let t = b.bytes(13)?;
                    // One precinct per line, Nikon's four components and code
                    // groups, the progression order and the single vertical
                    // level the packet layout below follows.
                    ensure!(
                        precinct_width == 0
                            && components == 4
                            && group == 4
                            && significance_group > 0
                            && slice_height > 0
                            && order >> 4 & 7 == 1
                            && decomposition & 15 == 1,
                        "unsupported High Efficiency layout"
                    );
                    ensure!(
                        width >= 8 && height >= 8 && width % 2 == 0 && height % 4 == 0,
                        "unsupported High Efficiency size {width}×{height}"
                    );
                    // The uniform inverse quantiser, signs in their own sub-packet,
                    // and insignificant groups taking the prediction: what every
                    // sample file uses, and all this decoder implements.
                    ensure!(
                        flags & 0x3f == 0b01_01_00,
                        "unsupported High Efficiency coding {flags:#04x}"
                    );
                    // Four fractional bits and 4-bit raw GCLIs, as every sample has:
                    // with GCLIs of at most 15, the wavelet stays well inside i32.
                    ensure!(
                        fq_br == 0x44,
                        "unsupported High Efficiency precision {fq_br:#04x}"
                    );
                    header = Some(Header {
                        width,
                        height,
                        slice_height,
                        group,
                        significance_group,
                        fraction_bits: u32::from(fq_br >> 4),
                        raw_gcli_bits: u32::from(fq_br & 15),
                        levels: Vec::new(),
                        weights: Vec::new(),
                        black: i64::from(t[3] & 15) << 8 | i64::from(t[4]),
                        gain: i64::from(t[7] & 3) << 16 | i64::from(t[8]) << 8 | i64::from(t[9]),
                        knee: i64::from(t[10]) << 8 | i64::from(t[11]),
                        first_slice: 0,
                    });
                }
                CDT => {
                    let h = header
                        .as_mut()
                        .context("JPEG XS component table before the picture header")?;
                    ensure!(
                        body.len() == 3 * components,
                        "unknown High Efficiency component table"
                    );
                    for _ in 0..components {
                        let (depth, sampling, decomposition) = (b.u8()?, b.u8()?, b.u8()?);
                        ensure!(
                            depth == 14 && sampling == 0x11,
                            "unknown High Efficiency component"
                        );
                        let (horizontal, vertical) = (decomposition >> 4, decomposition & 15);
                        ensure!(
                            (horizontal, vertical) == (0, 0) || vertical == 1 && horizontal >= 1,
                            "unsupported High Efficiency decomposition"
                        );
                        h.levels.push(u32::from(horizontal));
                    }
                }
                WGT => {
                    let h = header
                        .as_mut()
                        .context("JPEG XS weights before the picture header")?;
                    ensure!(body.len() % 2 == 0, "bad JPEG XS weights");
                    h.weights = body.chunks(2).map(|w| (w[0], w[1])).collect();
                }
                _ => {}
            }
        }
    }

    /// The offset of every precinct, in order, read from the slice and precinct
    /// headers.
    fn precincts(&self, data: &[u8]) -> Result<Vec<usize>> {
        let count = self.height / 4;
        let mut at = self.first_slice;
        let mut offsets = Vec::with_capacity(count);
        for p in 0..count {
            if p % self.slice_height == 0 {
                ensure!(
                    data.get(at..at + 2) == Some(&[0xff, 0x20][..]),
                    "JPEG XS slice header missing"
                );
                at += 6;
            }
            let length = data
                .get(at..at + 3)
                .context("High Efficiency codestream is truncated")?;
            offsets.push(at);
            let length =
                usize::from(length[0]) << 16 | usize::from(length[1]) << 8 | usize::from(length[2]);
            at += self.precinct_header_length() + length;
        }
        ensure!(
            data.get(at..at + 2) == Some(&EOC.to_be_bytes()[..]),
            "JPEG XS codestream does not end after its last precinct"
        );
        Ok(offsets)
    }

    fn bands(&self) -> usize {
        self.levels
            .iter()
            .map(|&l| if l == 0 { 1 } else { l as usize + 3 })
            .sum()
    }

    /// Length, quantisation, refinement, then two coding-mode bits per band.
    fn precinct_header_length(&self) -> usize {
        5 + (2 * self.bands()).div_ceil(8)
    }

    fn transfer_curve(&self) -> Transfer {
        let table = (0..=u16::MAX)
            .map(|i| {
                let d = i64::from(i) - self.knee;
                let q = if d < 0 {
                    -d * d
                } else {
                    let t = (d * self.gain + 32768) >> 16;
                    t * t
                };
                (self.black + ((q + 163_840) >> 18)).clamp(0, 16383) as u16
            })
            .collect();
        Transfer { table }
    }
}

/// Decoded values to raw values: the companding's inverse, from the 13 vendor bytes
/// of the picture header.
struct Transfer {
    table: Vec<u16>,
}
impl Transfer {
    #[inline]
    fn apply(&self, v: i32) -> u16 {
        self.table[(v + 32768).clamp(0, 65535) as usize]
    }
}

/// One band of one component: where its line goes in the coefficient rows.
#[derive(Debug, Clone)]
struct Band {
    component: usize,
    /// Its line's offset and width in the component's row: the vertical low-pass
    /// row for the low bands, the high-pass row for the high bands.
    offset: usize,
    width: usize,
    high: bool,
    gain: u8,
    priority: u8,
}

/// One packet: one line of each of its bands. Packets carry, for each line of the
/// precinct in turn, each component's bands on that line.
#[derive(Debug)]
struct Packet {
    bands: std::ops::Range<usize>,
    /// The precinct line (0 the low-pass, 1 the high-pass).
    line: usize,
}

struct Layout {
    /// One component's size: half the raw's both ways.
    width: usize,
    height: usize,
    levels: Vec<u32>,
    /// In packet order, which is also the weights' order.
    bands: Vec<Band>,
    packets: Vec<Packet>,
}
impl Layout {
    fn new(header: &Header) -> Result<Self> {
        let width = header.width / 2;
        let mut bands = Vec::new();
        let mut packets = Vec::new();
        for line in 0..2 {
            for (component, &levels) in header.levels.iter().enumerate() {
                let start = bands.len();
                let band = |offset, width, high| Band {
                    component,
                    offset,
                    width,
                    high,
                    gain: 0,
                    priority: 0,
                };
                let range = match (levels, line) {
                    // No wavelet: one band, with a line on both precinct lines and
                    // one weight.
                    (0, 0) => {
                        bands.push(band(0, width, false));
                        start..start + 1
                    }
                    (0, _) => {
                        let b = bands
                            .iter()
                            .position(|b| b.component == component)
                            .context("JPEG XS band missing")?;
                        b..b + 1
                    }
                    // The low-pass row, split horizontally `levels` times: the final
                    // low band first, then the high bands, widest last.
                    (_, 0) => {
                        let mut low = width;
                        let mut highs = Vec::new();
                        for _ in 0..levels {
                            highs.push(low / 2);
                            low -= low / 2;
                        }
                        let mut offset = 0;
                        for w in std::iter::once(low).chain(highs.into_iter().rev()) {
                            bands.push(band(offset, w, false));
                            offset += w;
                        }
                        start..bands.len()
                    }
                    // The high-pass row, split once.
                    _ => {
                        bands.push(band(0, width - width / 2, true));
                        bands.push(band(width - width / 2, width / 2, true));
                        start..bands.len()
                    }
                };
                packets.push(Packet { bands: range, line });
            }
        }
        ensure!(
            bands.len() == header.weights.len(),
            "JPEG XS weights do not match the bands"
        );
        for (band, &(gain, priority)) in bands.iter_mut().zip(&header.weights) {
            band.gain = gain;
            band.priority = priority;
        }
        Ok(Layout {
            width,
            height: header.height / 2,
            levels: header.levels.clone(),
            bands,
            packets,
        })
    }
}

/// Dequantised wavelet coefficients, a precinct's two rows (low-pass, high-pass) per
/// component, `width` each, all components of a precinct together.
struct Coefficients {
    width: usize,
    components: usize,
    rows: Vec<i32>,
}
impl Coefficients {
    fn new(layout: &Layout) -> Self {
        let precincts = layout.height / 2;
        Coefficients {
            width: layout.width,
            components: layout.levels.len(),
            rows: vec![0; precincts * 2 * layout.levels.len() * layout.width],
        }
    }
    fn precinct_len(&self) -> usize {
        2 * self.components * self.width
    }

    /// The four component planes, `width` by `height`, wavelet inverted.
    fn inverse_wavelet(mut self, layout: &Layout) -> [Vec<i32>; 4] {
        let (w, n) = (self.width, self.components);
        let precinct_len = self.precinct_len();
        // Horizontally, each row on its own.
        self.rows.par_chunks_mut(precinct_len).for_each(|precinct| {
            let mut scratch = vec![0; w];
            for (c, &levels) in layout.levels.iter().enumerate() {
                if levels == 0 {
                    continue;
                }
                let low = &mut precinct[c * w..(c + 1) * w];
                let mut widths = Vec::new();
                let mut len = w;
                for _ in 0..levels {
                    widths.push(len);
                    len -= len / 2;
                }
                for &len in widths.iter().rev() {
                    inverse_53(low, len, &mut scratch);
                }
                let high = &mut precinct[(n + c) * w..(n + c + 1) * w];
                inverse_53(high, w, &mut scratch);
            }
        });
        // Vertically: the plane's even rows from the low-pass rows, odd from the
        // high-pass ones, extended symmetrically at the top and bottom.
        let rows = &self.rows;
        let precincts = layout.height / 2;
        let row = |p: usize, c: usize, high: bool| {
            let at = p * precinct_len + (usize::from(high) * n + c) * w;
            &rows[at..at + w]
        };
        let plane = |c: usize| -> Vec<i32> {
            let mut plane = vec![0; w * layout.height];
            plane
                .par_chunks_mut(2 * w)
                .enumerate()
                .for_each(|(p, pair)| {
                    let (even, odd) = pair.split_at_mut(w);
                    if layout.levels[c] == 0 {
                        even.copy_from_slice(row(p, c, false));
                        odd.copy_from_slice(row(p, c, true));
                        return;
                    }
                    let even_at = |p: usize, x: usize| {
                        let above = row(p.saturating_sub(1), c, true)[x];
                        row(p, c, false)[x] - ((above + row(p, c, true)[x] + 2) >> 2)
                    };
                    let below = (p + 1).min(precincts - 1);
                    for x in 0..w {
                        let e = even_at(p, x);
                        let next = if below == p { e } else { even_at(below, x) };
                        even[x] = e;
                        odd[x] = row(p, c, true)[x] + ((e + next) >> 1);
                    }
                });
            plane
        };
        [plane(0), plane(1), plane(2), plane(3)]
    }
}

/// One level of the reversible 5/3 inverse on `line[..len]`, which holds the low half
/// (the larger when `len` is odd) then the high half; symmetric extension at both ends.
fn inverse_53(line: &mut [i32], len: usize, scratch: &mut [i32]) {
    if len < 2 {
        return;
    }
    let lows = len - len / 2;
    let highs = len / 2;
    let (low, high) = line[..len].split_at(lows);
    let out = &mut scratch[..len];
    let h = |k: usize| high[k.min(highs - 1)];
    for k in 0..lows {
        let left = if k == 0 { h(0) } else { h(k - 1) };
        out[2 * k] = low[k] - ((left + h(k) + 2) >> 2);
    }
    for k in 0..highs {
        let right = if 2 * k + 2 < len {
            out[2 * k + 2]
        } else {
            out[2 * k]
        };
        out[2 * k + 1] = high[k] + ((out[2 * k] + right) >> 1);
    }
    line[..len].copy_from_slice(out);
}

/// The star-tetrix inverse, in place: the planes (green at the red row's site, red,
/// green at the blue row's site, blue) come in as (Y, Cr, Δ, Cb). Samples beyond the
/// edges mirror on the full CFA grid.
fn inverse_star_tetrix(planes: &mut [Vec<i32>; 4], w: usize, h: usize) {
    let [y, cr, delta, cb] = planes;
    let left = |x: usize| x.saturating_sub(1);
    let right = |x: usize| (x + 1).min(w - 1);
    let up = |r: usize| r.saturating_sub(1);
    let down = |r: usize| (r + 1).min(h - 1);
    let at = |p: &[i32], r: usize, x: usize| p[r * w + x];
    // Y less half the mean of the four Δs diagonal to it.
    {
        let delta = &*delta;
        y.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let s = at(delta, up(r), x)
                    + at(delta, up(r), right(x))
                    + at(delta, r, x)
                    + at(delta, r, right(x));
                *v -= s >> 3;
            }
        });
    }
    // The second green: Δ less half the mean of the red and blue next to it, plus
    // the mean of the Ys diagonal to it.
    {
        let (y, cr, cb) = (&*y, &*cr, &*cb);
        delta.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let chroma = at(cr, r, x) + at(cr, down(r), x) + at(cb, r, left(x)) + at(cb, r, x);
                let greens =
                    at(y, r, left(x)) + at(y, r, x) + at(y, down(r), left(x)) + at(y, down(r), x);
                *v = *v - (chroma >> 3) + (greens >> 2);
            }
        });
    }
    // The first green: Y less half the mean of the red and blue next to it.
    {
        let (cr, cb) = (&*cr, &*cb);
        y.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
            for (x, v) in row.iter_mut().enumerate() {
                let chroma = at(cr, r, x) + at(cr, r, right(x)) + at(cb, up(r), x) + at(cb, r, x);
                *v -= chroma >> 3;
            }
        });
    }
    // Red and blue regain the mean of the four greens next to them.
    let (g1, g2) = (&*y, &*delta);
    cr.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let greens = at(g1, r, left(x)) + at(g1, r, x) + at(g2, up(r), x) + at(g2, r, x);
            *v += greens >> 2;
        }
    });
    cb.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let greens = at(g1, r, x) + at(g1, down(r), x) + at(g2, r, x) + at(g2, r, right(x));
            *v += greens >> 2;
        }
    });
}

/// Bits, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    /// The next bit's position.
    at: usize,
    end: usize,
}
impl<'a> Bits<'a> {
    fn new(data: &'a [u8], start: usize, end: usize) -> Self {
        Bits {
            data,
            at: start * 8,
            end: end * 8,
        }
    }
    /// Up to 57 bits from the current position, zero past the end.
    #[inline]
    fn peek(&self) -> u64 {
        let byte = self.at / 8;
        let mut word = [0u8; 8];
        let available = self.end / 8 - byte.min(self.end / 8);
        let n = available.min(8);
        word[..n].copy_from_slice(&self.data[byte..byte + n]);
        u64::from_be_bytes(word) << (self.at % 8)
    }
    #[inline]
    fn read(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        ensure!(
            self.at + n as usize <= self.end,
            "High Efficiency packet is truncated"
        );
        let v = (self.peek() >> (64 - n)) as u32;
        self.at += n as usize;
        Ok(v)
    }
    /// A unary code: ones up to a zero.
    #[inline]
    fn unary(&mut self) -> Result<u32> {
        let ones = self.peek().leading_ones();
        ensure!(ones < 32, "bad High Efficiency GCLI code");
        ensure!(
            self.at + (ones as usize) < self.end,
            "High Efficiency packet is truncated"
        );
        self.at += ones as usize + 1;
        Ok(ones)
    }
    fn align(&mut self) {
        self.at = self.at.next_multiple_of(8);
    }
    fn byte(&self) -> usize {
        self.at / 8
    }
    fn skip_to(&mut self, byte: usize) -> Result<()> {
        ensure!(
            byte * 8 >= self.at && byte * 8 <= self.end,
            "High Efficiency sub-packet overruns its length"
        );
        self.at = byte * 8;
        Ok(())
    }
}

/// Entropy decoding, a slice per task: vertical GCLI prediction runs within a slice.
fn decode_slices(
    data: &[u8],
    header: &Header,
    layout: &Layout,
    precincts: &[usize],
    coefficients: &mut Coefficients,
    cancel: &AtomicBool,
) -> Result<()> {
    let precinct_len = coefficients.precinct_len();
    let (w, n) = (coefficients.width, coefficients.components);
    coefficients
        .rows
        .par_chunks_mut(precinct_len * header.slice_height)
        .zip(precincts.par_chunks(header.slice_height))
        .try_for_each(|(rows, offsets)| {
            let mut state = SliceState::new(layout);
            for (precinct, &offset) in rows.chunks_mut(precinct_len).zip(offsets) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                decode_precinct(data, offset, header, layout, &mut state, precinct, w, n)?;
            }
            Ok(())
        })
}

/// The previous line's GCLIs and truncation position, per band, for vertical
/// prediction; none at the start of a slice.
struct SliceState {
    previous: Vec<Option<(Vec<u8>, u8)>>,
}
impl SliceState {
    fn new(layout: &Layout) -> Self {
        SliceState {
            previous: vec![None; layout.bands.len()],
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_precinct(
    data: &[u8],
    offset: usize,
    header: &Header,
    layout: &Layout,
    state: &mut SliceState,
    rows: &mut [i32],
    w: usize,
    n: usize,
) -> Result<()> {
    let head = &data[offset..offset + header.precinct_header_length()];
    let length = usize::from(head[0]) << 16 | usize::from(head[1]) << 8 | usize::from(head[2]);
    let (quantisation, refinement) = (head[3], head[4]);
    let mode = |b: usize| head[5 + b / 4] >> (6 - 2 * (b % 4)) & 3;
    let start = offset + head.len();
    let end = start + length;
    ensure!(end <= data.len(), "High Efficiency codestream is truncated");
    let mut bits = Bits::new(data, start, end);
    let group = header.group;
    let ss = header.significance_group;
    let gtli = |band: &Band| {
        let refine = u8::from(band.priority < refinement);
        quantisation
            .saturating_sub(band.gain)
            .saturating_sub(refine)
            .min(15)
    };
    for packet in &layout.packets {
        let head = bits.read(32)?;
        let tail = bits.read(24)?;
        let raw = head >> 31 != 0;
        let data_len = (head >> 11 & 0xf_ffff) as usize;
        let gcli_len = ((head & 0x7ff) << 9 | tail >> 15) as usize;
        let sign_len = (tail & 0x7fff) as usize;
        // Significance flags, one per group of `ss` code groups, for each band that
        // codes them; a set flag marks the group insignificant.
        let mut significance: Vec<Option<Vec<bool>>> = Vec::new();
        if !raw {
            for b in packet.bands.clone() {
                let band = &layout.bands[b];
                significance.push(if mode(b) & 2 != 0 {
                    let groups = band.width.div_ceil(group).div_ceil(ss);
                    Some(
                        (0..groups)
                            .map(|_| bits.read(1).map(|v| v != 0))
                            .collect::<Result<_>>()?,
                    )
                } else {
                    None
                });
            }
            bits.align();
        }
        // GCLIs: one bit stream across the packet's bands.
        let gcli_start = bits.byte();
        let mut gclis: Vec<Vec<u8>> = Vec::with_capacity(packet.bands.len());
        for (k, b) in packet.bands.clone().enumerate() {
            let band = &layout.bands[b];
            let t = gtli(band);
            let groups = band.width.div_ceil(group);
            let mut line = vec![0u8; groups];
            if raw {
                for g in line.iter_mut() {
                    *g = bits.read(header.raw_gcli_bits)? as u8;
                }
            } else {
                let insignificant = |g: usize| significance[k].as_ref().is_some_and(|s| s[g / ss]);
                match (&state.previous[b], mode(b) & 1 != 0) {
                    (Some((top, top_gtli)), true) => {
                        let floor = (*top_gtli).max(t);
                        for (g, out) in line.iter_mut().enumerate() {
                            let m = top[g].max(floor);
                            *out = if insignificant(g) {
                                m
                            } else {
                                let x = bits.unary()? as i32;
                                let room = (i32::from(m) - i32::from(t)).max(0);
                                let delta = if x > 2 * room {
                                    x - room
                                } else if x & 1 == 1 {
                                    -(x + 1) / 2
                                } else {
                                    x / 2
                                };
                                u8::try_from(i32::from(m) + delta)
                                    .context("bad High Efficiency GCLI")?
                            };
                        }
                    }
                    _ => {
                        for (g, out) in line.iter_mut().enumerate() {
                            *out = if insignificant(g) {
                                t
                            } else {
                                u8::try_from(u32::from(t) + bits.unary()?)
                                    .context("bad High Efficiency GCLI")?
                            };
                        }
                    }
                }
            }
            // Magnitudes have 15 bits.
            ensure!(line.iter().all(|&g| g <= 15), "bad High Efficiency GCLI");
            gclis.push(line);
        }
        bits.align();
        bits.skip_to(gcli_start + gcli_len)?;
        // Magnitudes: per code group with GCLI above the truncation position, that
        // many bit planes of the group's four coefficients, most significant first.
        let data_start = bits.byte();
        let mut values: Vec<Vec<u32>> = Vec::with_capacity(packet.bands.len());
        let mut negative: Vec<Vec<bool>> = Vec::with_capacity(packet.bands.len());
        for (k, b) in packet.bands.clone().enumerate() {
            let band = &layout.bands[b];
            let t = gtli(band);
            let groups = band.width.div_ceil(group);
            let mut v = vec![0u32; groups * group];
            for g in 0..groups {
                let gcli = gclis[k][g];
                if gcli <= t {
                    continue;
                }
                for _ in 0..gcli - t {
                    let plane = bits.read(group as u32)?;
                    for j in 0..group {
                        v[g * group + j] = v[g * group + j] << 1 | plane >> (group - 1 - j) & 1;
                    }
                }
                for j in 0..group {
                    v[g * group + j] <<= t;
                }
            }
            negative.push(vec![false; v.len()]);
            values.push(v);
        }
        bits.align();
        bits.skip_to(data_start + data_len)?;
        // Signs of the nonzero coefficients, the group padding past a band's end
        // included.
        let sign_start = bits.byte();
        for k in 0..packet.bands.len() {
            for (v, s) in values[k].iter().zip(negative[k].iter_mut()) {
                if *v != 0 {
                    *s = bits.read(1)? != 0;
                }
            }
        }
        bits.align();
        bits.skip_to(sign_start + sign_len)?;
        // Dequantise into the precinct's rows, and keep the GCLIs for the next line.
        for (k, b) in packet.bands.clone().enumerate() {
            let band = &layout.bands[b];
            let t = gtli(band);
            let row = usize::from(band.high || packet.line == 1) * n + band.component;
            let out = &mut rows[row * w + band.offset..row * w + band.offset + band.width];
            for (i, o) in out.iter_mut().enumerate() {
                let gcli = gclis[k][i / group];
                let mut m = values[k][i];
                // Uniform inverse quantisation: the dropped bit planes repeat the
                // coded ones.
                if t > 0 && gcli > t && m != 0 {
                    let scale = u32::from(gcli - t) + 1;
                    let mut rest = m;
                    m = 0;
                    while rest > 0 {
                        m += rest;
                        rest >>= scale;
                    }
                }
                let v = (m as i32) << header.fraction_bits;
                *o = if negative[k][i] { -v } else { v };
            }
            state.previous[b] = Some((std::mem::take(&mut gclis[k]), t));
        }
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {
    /// The vendor bytes of every sample file: black 1008, gain 84429 (Q16), knee
    /// 16255.
    const VENDOR: [u8; 13] = [
        0x50, 0x88, 0x70, 0x83, 0xf0, 0x15, 0x23, 0xd1, 0x49, 0xcd, 0x3f, 0x7f, 0x07,
    ];

    struct BitWriter {
        bytes: Vec<u8>,
        bits: usize,
    }
    impl BitWriter {
        fn put(&mut self, value: u64, n: usize) {
            for i in (0..n).rev() {
                if self.bits.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                let bit = (value >> i & 1) as u8;
                *self.bytes.last_mut().unwrap() |= bit << (7 - self.bits % 8);
                self.bits += 1;
            }
        }
        fn align(&mut self) {
            self.bits = self.bits.next_multiple_of(8);
        }
    }

    /// [`blank`] with the sample files' vendor bytes.
    pub fn blank_codestream(width: u16, height: u16) -> Vec<u8> {
        blank(width, height, VENDOR)
    }
    /// What every value of a blank codestream decodes to: zero, biased to 32768,
    /// past the knee, ((32768 − 16255) × 84429 / 2¹⁶)² in 18 fractional bits,
    /// rounded, on the black.
    pub fn blank_value() -> u16 {
        let t: i64 = (16513 * 84429 + 32768) >> 16;
        (1008 + ((t * t + 163_840) >> 18)) as u16
    }
    /// A codestream laid out as Nikon's, `width` by `height`, whose every
    /// coefficient is zero: each packet marks all of its significance groups
    /// insignificant and codes nothing else.
    fn blank(width: u16, height: u16, vendor: [u8; 13]) -> Vec<u8> {
        coded(width, height, vendor, None)
    }
    /// [`blank`], except that with `first_gcli` the first code group of the first
    /// band is significant, with that GCLI and nothing after it.
    fn coded(width: u16, height: u16, vendor: [u8; 13], first_gcli: Option<u32>) -> Vec<u8> {
        let mut s = vec![0xff, 0x10, 0xff, 0x50, 0, 34];
        s.extend(b"CONTACT_INTOPIX_");
        s.extend([0; 16]);
        s.extend([0xff, 0x12, 0, 39, 0, 0, 0, 0, 0, 0, 0, 0]);
        s.extend(width.to_be_bytes());
        s.extend(height.to_be_bytes());
        s.extend([0, 0, 0, 16, 4, 4, 8, 18, 0x44, 0x10, 0x51, 0x14]);
        s.extend(vendor);
        s.extend([0xff, 0x13, 0, 14]);
        s.extend([
            14, 0x11, 0x51, 14, 0x11, 0x51, 14, 0x11, 0x00, 14, 0x11, 0x51,
        ]);
        s.extend([0xff, 0x14, 0, 52]);
        s.extend((0..25u8).flat_map(|b| [0, b]));
        let header = super::Header::parse(&[s.clone(), vec![0xff, 0x20]].concat()).unwrap();
        let layout = super::Layout::new(&header).unwrap();
        let precincts = usize::from(height) / 4;
        for p in 0..precincts {
            if p % 16 == 0 {
                s.extend([0xff, 0x20, 0, 4]);
                s.extend(((p / 16) as u16).to_be_bytes());
            }
            let mut payload = BitWriter {
                bytes: Vec::new(),
                bits: 0,
            };
            for (k, packet) in layout.packets.iter().enumerate() {
                let gcli = first_gcli.filter(|_| p == 0 && k == 0);
                let gcli_len = gcli.map_or(0, |x| (x as u64 + 1).div_ceil(8));
                payload.put(gcli_len << 15, 56);
                for (i, band) in layout.bands[packet.bands.clone()].iter().enumerate() {
                    let groups = band.width.div_ceil(4).div_ceil(8);
                    // Set flags mark groups insignificant; the first group's is clear.
                    let flags = if gcli.is_some() && i == 0 {
                        (1 << (groups - 1)) - 1
                    } else {
                        u64::MAX
                    };
                    payload.put(flags, groups);
                }
                payload.align();
                if let Some(x) = gcli {
                    payload.put(u64::MAX, x as usize);
                    payload.put(0, 1);
                    payload.align();
                }
            }
            let length = payload.bytes.len() as u32;
            s.extend(&length.to_be_bytes()[1..]);
            // Quantisation 0, refinement 0, every band significance-coded and
            // vertically predicted.
            s.extend([0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc0]);
            s.extend(payload.bytes);
        }
        s.extend([0xff, 0x11]);
        s
    }

    #[test]
    fn a_blank_codestream_decodes_to_the_transfer_curve_at_zero() {
        let (width, height) = (64, 32);
        let stream = blank_codestream(width as u16, height as u16);
        let mut out = vec![0u16; width * height];
        super::decode(&stream, &mut out, width, height, width).unwrap();
        assert_eq!(blank_value(), 2734);
        assert!(out.iter().all(|&v| v == blank_value()), "{:?}", &out[..8]);
    }

    #[test]
    fn decoding_fills_only_the_image_columns_of_each_row() {
        let (width, height, pitch) = (64, 32, 70);
        let stream = blank(width as u16, height as u16, VENDOR);
        let mut out = vec![7u16; pitch * height];
        super::decode(&stream, &mut out, width, height, pitch).unwrap();
        for row in out.chunks(pitch) {
            assert!(row[..width].iter().all(|&v| v != 7));
            assert!(row[width..].iter().all(|&v| v == 7));
        }
        // A buffer of another size is refused, not overrun.
        assert!(super::decode(&stream, &mut out, 62, height, pitch).is_err());
        assert!(super::decode(&stream, &mut out[..pitch * 31], width, height, pitch).is_err());
    }

    #[test]
    fn the_transfer_curve_rounds_the_gain_half_up() {
        // Gain 65905: at index 49023 the scaled value is 32952.5, which Adobe DNG
        // Converter rounds up (raw 5151, not 5150).
        let mut vendor = VENDOR;
        vendor[7..10].copy_from_slice(&[0xd1, 0x01, 0x71]);
        let header = super::Header::parse(&blank(64, 32, vendor)).unwrap();
        assert_eq!(
            (header.black, header.gain, header.knee),
            (1008, 65905, 16255)
        );
        let curve = header.transfer_curve();
        assert_eq!(curve.apply(49023 - 32768), 5151);
        // Below the knee the value falls quadratically, unscaled and rounded down
        // (−10⁶ + 163840 is −3.2 in 18 fractional bits).
        assert_eq!(curve.apply(16255 - 32768 - 1000), 1008 - 4);
        // The index clamps to 16 bits, and the value to 14.
        assert_eq!(curve.apply(i32::MIN / 2), 0);
        assert_eq!(curve.apply(i32::MAX / 2), curve.apply(32767));
        assert_eq!(curve.apply(32767), 10377);
        let header = super::Header::parse(&blank(64, 32, VENDOR)).unwrap();
        assert_eq!(header.transfer_curve().apply(32767), 16383);
    }

    #[test]
    fn a_gcli_beyond_15_bits_is_refused() {
        let (width, height) = (64, 32);
        let mut out = vec![0u16; width * height];
        let gcli = |x| coded(width as u16, height as u16, VENDOR, Some(x));
        // The packet is well formed, so its GCLI is read: 15 is accepted (and the
        // data it calls for is missing), more is refused before the data is read.
        let error = super::decode(&gcli(15), &mut out, width, height, width).unwrap_err();
        assert!(!error.to_string().contains("GCLI"), "{error}");
        for x in [16, 31] {
            let error = super::decode(&gcli(x), &mut out, width, height, width).unwrap_err();
            assert!(error.to_string().contains("GCLI"), "{x}: {error}");
        }
    }

    #[test]
    fn decoding_stops_when_cancelled() {
        let (width, height) = (64, 32);
        let stream = blank(width as u16, height as u16, VENDOR);
        let mut out = vec![0u16; width * height];
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancel = AtomicBool::new(true);
        assert_eq!(
            super::decode_cancellable(&stream, &mut out, width, height, width, &cancel),
            Err(crate::error::Error::Cancelled)
        );
        cancel.store(false, Ordering::Relaxed);
        super::decode_cancellable(&stream, &mut out, width, height, width, &cancel).unwrap();
        assert!(out.iter().all(|&v| v == blank_value()));
    }

    #[test]
    fn other_precisions_are_refused() {
        let mut stream = blank(64, 32, VENDOR);
        // The picture header's Fq and Br byte.
        assert_eq!(stream[62], 0x44);
        for value in [0xf4, 0x54, 0x45] {
            stream[62] = value;
            assert!(super::Header::parse(&stream).is_err(), "{value:#x}");
        }
    }

    #[test]
    fn damaged_codestreams_fail_without_panicking() {
        let (width, height) = (64, 32);
        let stream = blank(width as u16, height as u16, VENDOR);
        let mut out = vec![0u16; width * height];
        for len in 0..stream.len() {
            assert!(
                super::decode(&stream[..len], &mut out, width, height, width).is_err(),
                "decoded {len} bytes"
            );
        }
        // Any one byte changed: decoded or refused, never a panic.
        for at in 0..stream.len() {
            for value in [0x00, 0x7f, 0xff] {
                let mut damaged = stream.clone();
                damaged[at] = value;
                let _ = super::decode(&damaged, &mut out, width, height, width);
            }
        }
    }
}
