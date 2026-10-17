//! Lossless JPEG (ITU T.81 process 14, "LJ92"): the frame and scan headers,
//! Huffman tables and restart intervals, and the seven predictors, each
//! component predicted from its own neighbouring samples. The output is the
//! stream's MCU rows as coded: each row is [`Header::mcus`] MCUs of every
//! component's samples (a component sampled `h × v` contributes its `h × v`
//! block, row by row, to each MCU, in frame order). Where those samples
//! belong in the image is the container's business (Canon's slices, DNG
//! tiles, small-RAW YCbCr).
use super::bits::Msb;
use super::huffman::Huffman;
use crate::error::{Error, Result, corrupt};
use std::sync::atomic::{AtomicBool, Ordering};

/// A component of the frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Component {
    pub id: u8,
    pub h: u8,
    pub v: u8,
}

/// The headers of a lossless JPEG stream.
#[derive(Debug)]
pub(crate) struct Header {
    pub precision: u8,
    pub width: u16,
    pub height: u16,
    pub components: Vec<Component>,
    /// Where the entropy-coded data of the (first) scan starts.
    pub(crate) scan: usize,
    predictor: u8,
    point_transform: u8,
    /// Per component of the scan (in frame order), its Huffman table.
    tables: Vec<usize>,
    huffman: Vec<Option<Huffman>>,
    restart: u16,
    /// Whether the first row of a component's block predicts from the first
    /// row of the block above it, not from the row just above (Sony's small
    /// RAWs sampled 2 × 2; our measurements). T.81's is the row just above.
    pub block_rows: bool,
    /// Whether a component's `h × v` samples of an MCU are one line of
    /// `h·v` samples, predicted along it from the left and from the line of
    /// the MCU row above (Canon's small RAWs; our measurements: T.81's
    /// two-dimensional prediction of their second row decodes noise).
    pub lines: bool,
}

impl std::fmt::Debug for Huffman {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Huffman")
    }
}

impl Header {
    /// Samples per MCU: the sum of every component's `h × v`.
    pub fn samples_per_mcu(&self) -> usize {
        self.components
            .iter()
            .map(|c| usize::from(c.h) * usize::from(c.v))
            .sum()
    }
    /// MCUs across and down: the frame's width and height are in samples of
    /// the most finely sampled component (T.81 A.1.1).
    pub fn mcus(&self) -> (usize, usize) {
        let h = self.components.iter().map(|c| c.h).max().unwrap_or(1);
        let v = self.components.iter().map(|c| c.v).max().unwrap_or(1);
        (
            usize::from(self.width).div_ceil(usize::from(h)),
            usize::from(self.height).div_ceil(usize::from(v)),
        )
    }
}

/// Reads the headers up to the first scan.
pub(crate) fn header(data: &[u8]) -> Result<Header> {
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return Err(corrupt("lossless JPEG without SOI"));
    }
    let mut at = 2;
    let mut frame: Option<(u8, u16, u16, Vec<Component>)> = None;
    let mut huffman: Vec<Option<Huffman>> = (0..4).map(|_| None).collect();
    let mut restart = 0u16;
    loop {
        // Fill bytes before a marker are allowed.
        while at < data.len() && data[at] == 0xff && data.get(at + 1) == Some(&0xff) {
            at += 1;
        }
        if at + 4 > data.len() || data[at] != 0xff {
            return Err(corrupt("lossless JPEG headers end early"));
        }
        let marker = data[at + 1];
        let len = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        let body = data
            .get(at + 4..at + 2 + len)
            .filter(|_| len >= 2)
            .ok_or_else(|| corrupt("lossless JPEG segment"))?;
        match marker {
            // Some writers mark lossless frames as baseline or extended
            // (Blackmagic): the frame header is the same.
            0xc0..=0xc3 => {
                if body.len() < 6 {
                    return Err(corrupt("SOF3"));
                }
                let n = usize::from(body[5]);
                if n == 0 || n > 4 || body.len() < 6 + 3 * n {
                    return Err(corrupt("SOF3 components"));
                }
                let components = (0..n)
                    .map(|i| Component {
                        id: body[6 + 3 * i],
                        h: (body[7 + 3 * i] >> 4).max(1),
                        v: (body[7 + 3 * i] & 15).max(1),
                    })
                    .collect();
                frame = Some((
                    body[0],
                    u16::from_be_bytes([body[1], body[2]]),
                    u16::from_be_bytes([body[3], body[4]]),
                    components,
                ));
            }
            0xc5..=0xcf if marker != 0xc8 && marker != 0xcc => {
                return Err(Error::Unsupported(format!("JPEG process {marker:#x}")));
            }
            0xc4 => {
                let mut b = body;
                while b.len() >= 17 {
                    let class = usize::from(b[0] & 15);
                    let counts: [u8; 16] = b[1..17].try_into().map_err(|_| corrupt("DHT"))?;
                    let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    let symbols = b.get(17..17 + n).ok_or_else(|| corrupt("DHT"))?;
                    if class < 4 {
                        huffman[class] = Some(Huffman::new(&counts, symbols)?);
                    }
                    b = &b[17 + n..];
                }
            }
            0xdd => {
                restart = u16::from_be_bytes([
                    *body.first().ok_or_else(|| corrupt("DRI"))?,
                    *body.get(1).ok_or_else(|| corrupt("DRI"))?,
                ]);
            }
            0xda => {
                let (precision, height, width, components) =
                    frame.ok_or_else(|| corrupt("SOS before SOF3"))?;
                let n = usize::from(*body.first().ok_or_else(|| corrupt("SOS"))?);
                if n != components.len() || body.len() < 1 + 2 * n + 3 {
                    return Err(Error::Unsupported(
                        "lossless JPEG with several scans".into(),
                    ));
                }
                // Leaf's MOS number the scan's components from one and the
                // frame's from zero (our measurement): unless every component
                // of the frame is named in the scan, the scan's are the frame's in
                // order.
                let named = components
                    .iter()
                    .all(|c| (0..n).any(|i| body[1 + 2 * i] == c.id));
                let mut tables = Vec::with_capacity(n);
                for (k, c) in components.iter().enumerate() {
                    let table = (0..n)
                        .find(|&i| {
                            if named {
                                body[1 + 2 * i] == c.id
                            } else {
                                i == k
                            }
                        })
                        .map(|i| usize::from(body[2 + 2 * i] >> 4))
                        .ok_or_else(|| corrupt("SOS component"))?;
                    if table >= 4 || huffman[table].is_none() {
                        return Err(corrupt("SOS Huffman table"));
                    }
                    tables.push(table);
                }
                let predictor = body[1 + 2 * n];
                let point_transform = body[3 + 2 * n] & 15;
                if !(1..=7).contains(&predictor)
                    || !(2..=16).contains(&precision)
                    || point_transform >= precision
                {
                    return Err(corrupt(
                        "lossless JPEG predictor, precision or point transform",
                    ));
                }
                return Ok(Header {
                    precision,
                    width,
                    height,
                    components,
                    scan: at + 2 + len,
                    predictor,
                    point_transform,
                    tables,
                    huffman,
                    restart,
                    block_rows: false,
                    lines: false,
                });
            }
            0xd9 => return Err(corrupt("EOI before a scan")),
            _ => {}
        }
        at += 2 + len;
    }
}

/// Decodes the stream: MCU rows of MCUs × `samples_per_mcu` samples.
#[cfg(test)]
pub(crate) fn decode(data: &[u8], cancel: &AtomicBool) -> Result<(Header, Vec<u16>)> {
    let h = header(data)?;
    let mut out = Vec::new();
    decode_into(&h, data, cancel, &mut out)?;
    Ok((h, out))
}

/// Decodes the stream whose headers are `h` into `out` (resized). Some
/// encoders (action cameras' DNGs) write 16 bits after a difference of
/// length 16, which T.81 codes with none. A stream is read as T.81 says
/// unless that fails or leaves a byte or more of its data unread, and reading
/// the 16 bits then decodes it and consumes it exactly.
pub(crate) fn decode_into(
    h: &Header,
    data: &[u8],
    cancel: &AtomicBool,
    out: &mut Vec<u16>,
) -> Result<()> {
    let spec = decode_with(h, data, cancel, out, false);
    // Only differences of length 16 read differently.
    if let Ok(read) = spec
        && (read.exact() || read.sixteens == 0)
    {
        return Ok(());
    }
    let mut other = Vec::new();
    match (decode_with(h, data, cancel, &mut other, true), spec) {
        (Ok(read), _) if read.exact() => {
            *out = other;
            Ok(())
        }
        // Neither reading consumes the stream exactly (padding, truncation):
        // the specification's, when it decodes.
        (_, Ok(_)) => Ok(()),
        (_, Err(e)) => Err(e),
    }
}

/// What reading a stream left: the bits of its data not consumed, whether
/// it read past them, and how many differences of length 16 it had.
#[derive(Clone, Copy)]
struct Read {
    unread: u64,
    overrun: bool,
    sixteens: u64,
}

impl Read {
    /// Whether the reading consumed the stream's data exactly: all but its
    /// padding, and nothing past it.
    fn exact(&self) -> bool {
        self.unread < 8 && !self.overrun
    }
}

/// A sample of an MCU: its Huffman table and its place in its component's
/// block of `bw × bh` samples, which starts `offset` samples into the MCU.
struct Slot<'a> {
    table: &'a Huffman,
    offset: usize,
    bw: usize,
    bh: usize,
    i: usize,
    j: usize,
}

/// Decodes the stream.
fn decode_with(
    h: &Header,
    data: &[u8],
    cancel: &AtomicBool,
    out: &mut Vec<u16>,
    sixteen: bool,
) -> Result<Read> {
    let mut sixteens = 0;
    let per_mcu = h.samples_per_mcu();
    let (across, rows) = h.mcus();
    let row = across * per_mcu;
    crate::io::samples(across as u32, rows as u32, per_mcu as u32)?;
    out.clear();
    out.resize(row * rows, 0);
    // Each sample of an MCU in order: its Huffman table, and where it is in
    // its component's block (offset of the component in the MCU, the block's
    // width and height, the sample's row and column in it).
    let mut slots = Vec::with_capacity(per_mcu);
    let mut offset = 0;
    for (c, &t) in h.components.iter().zip(&h.tables) {
        let table = h.huffman[t]
            .as_ref()
            .ok_or_else(|| corrupt("Huffman table"))?;
        let (bw, bh) = if h.lines {
            (usize::from(c.h) * usize::from(c.v), 1)
        } else {
            (usize::from(c.h), usize::from(c.v))
        };
        for i in 0..bh {
            for j in 0..bw {
                slots.push(Slot {
                    table,
                    offset,
                    bw,
                    bh,
                    i,
                    j,
                });
            }
        }
        offset += bw * bh;
    }
    let shift = u32::from(h.point_transform);
    let initial = 1i32 << (u32::from(h.precision) - shift - 1);
    let mask = (1i32 << h.precision) - 1;
    let mut bits = Msb::jpeg(&data[h.scan.min(data.len())..]);
    let restart = usize::from(h.restart);
    let mut mcus_left = restart;
    let mut fresh = true;
    // Where a sample is: MCU row `y`, column `x`, row `i` and column `j` of
    // its component's block.
    let at = |s: &Slot, y: usize, x: usize, i: usize, j: usize| {
        y * row + x * per_mcu + s.offset + i * s.bw + j
    };
    for y in 0..rows {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        // Whether the MCU row above is a line to predict from.
        let mut above = y > 0;
        for x in 0..across {
            if restart > 0 {
                if mcus_left == 0 {
                    bits.restart();
                    mcus_left = restart;
                    fresh = true;
                    // A restart interval starting a row starts its first line:
                    // the row predicts from the left only.
                    if x == 0 {
                        above = false;
                    }
                }
                mcus_left -= 1;
            }
            for s in &slots {
                let diff = s.table.difference(&mut bits, sixteen)?;
                // Only a difference of length 16 reaches 32768.
                sixteens += u64::from(diff.abs() >= 32768);
                // Each component is predicted within its own samples: the
                // neighbours left (a), above (b) and above left (c).
                let left = |i: usize| {
                    if s.j > 0 {
                        Some(at(s, y, x, i, s.j - 1))
                    } else if x > 0 {
                        Some(at(s, y, x - 1, i, s.bw - 1))
                    } else {
                        None
                    }
                };
                let (ra, rb, rc) = if s.i > 0 {
                    (left(s.i), Some(at(s, y, x, s.i - 1, s.j)), left(s.i - 1))
                } else if above {
                    let row = if h.block_rows { 0 } else { s.bh - 1 };
                    let up = |x: usize, j: usize| at(s, y - 1, x, row, j);
                    let rc = if s.j > 0 {
                        Some(up(x, s.j - 1))
                    } else if x > 0 {
                        Some(up(x - 1, s.bw - 1))
                    } else {
                        None
                    };
                    (left(0), Some(up(x, s.j)), rc)
                } else {
                    (left(0), None, None)
                };
                let v = |i: Option<usize>| i.map(|i| i32::from(out[i]));
                let prediction = match (v(ra), v(rb), v(rc)) {
                    // The first sample of the stream or of a restart interval.
                    _ if fresh && s.i == 0 && s.j == 0 => initial,
                    (Some(ra), Some(rb), Some(rc)) => match h.predictor {
                        1 => ra,
                        2 => rb,
                        3 => rc,
                        4 => ra + rb - rc,
                        5 => ra + ((rb - rc) >> 1),
                        6 => rb + ((ra - rc) >> 1),
                        _ => (ra + rb) >> 1,
                    },
                    // The first line predicts from the left, the first column
                    // from above.
                    (Some(ra), None, _) => ra,
                    (None, Some(rb), _) => rb,
                    _ => initial,
                };
                let i = at(s, y, x, s.i, s.j);
                out[i] = ((prediction + diff) & mask) as u16;
            }
            // Only the first MCU of the stream or of a restart interval starts
            // from the initial prediction.
            fresh = false;
        }
    }
    if shift > 0 {
        for v in out.iter_mut() {
            *v <<= shift;
        }
    }
    Ok(Read {
        unread: bits.unread(),
        overrun: bits.overrun(),
        sixteens,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_scan_naming_none_of_the_frame_s_components_takes_them_in_order() {
        let samples: Vec<u16> = (0..16).map(|v| v * 100).collect();
        let mut jpeg = encode(&samples, 4, 16, 1);
        // The frame's component (after SOF3's marker, length, precision,
        // size and count) renumbered: the scan names it no more.
        let sof = jpeg.windows(2).position(|w| w == [0xff, 0xc3]).unwrap();
        jpeg[sof + 10] = jpeg[sof + 10].wrapping_add(7);
        let (_, out) = decode(&jpeg, &AtomicBool::new(false)).unwrap();
        assert_eq!(out, samples);
    }

    /// A lossless JPEG of `width × height` samples of one component with
    /// `predictor`, Huffman-coding every difference with a table of lengths
    /// 0 to 16 (symbol n has a 5-bit code n), written independently of the
    /// decoder: samples in, differences computed here from T.81's predictors.
    pub(crate) fn encode(samples: &[u16], width: usize, precision: u8, predictor: u8) -> Vec<u8> {
        let height = samples.len() / width;
        let at = |x: usize, y: usize| i32::from(samples[y * width + x]);
        let mut bits: Vec<bool> = Vec::new();
        let put = |v: u32, n: u32, bits: &mut Vec<bool>| {
            for i in (0..n).rev() {
                bits.push(v >> i & 1 == 1);
            }
        };
        for y in 0..height {
            for x in 0..width {
                let p = if x == 0 && y == 0 {
                    1 << (precision - 1)
                } else if y == 0 {
                    at(x - 1, 0)
                } else if x == 0 {
                    at(0, y - 1)
                } else {
                    let (a, b, c) = (at(x - 1, y), at(x, y - 1), at(x - 1, y - 1));
                    match predictor {
                        1 => a,
                        2 => b,
                        3 => c,
                        4 => a + b - c,
                        5 => a + ((b - c) >> 1),
                        6 => b + ((a - c) >> 1),
                        _ => (a + b) >> 1,
                    }
                };
                let m = (1i32 << precision) - 1;
                let mut d = (at(x, y) - p) & m;
                if d >= 1 << (precision - 1) {
                    d -= 1 << precision;
                }
                let len = if d == 0 {
                    0
                } else {
                    32 - d.unsigned_abs().leading_zeros()
                };
                put(len, 5, &mut bits);
                if len > 0 && len < 16 {
                    let v = if d < 0 {
                        (d + (1 << len) - 1) as u32
                    } else {
                        d as u32
                    };
                    put(v, len, &mut bits);
                }
            }
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(true);
        }
        let mut entropy = Vec::new();
        for byte in bits.chunks(8) {
            let b = byte.iter().fold(0u8, |a, &x| a << 1 | u8::from(x));
            entropy.push(b);
            if b == 0xff {
                entropy.push(0);
            }
        }
        let mut f = vec![0xff, 0xd8];
        // DHT: 17 codes of 5 bits, symbols 0..=16.
        let mut dht = vec![0x00];
        let mut counts = [0u8; 16];
        counts[4] = 17;
        dht.extend(counts);
        dht.extend(0..=16u8);
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        seg(&mut f, 0xc4, &dht);
        let mut sof = vec![precision];
        sof.extend((height as u16).to_be_bytes());
        sof.extend((width as u16).to_be_bytes());
        sof.extend([1, 1, 0x11, 0]);
        seg(&mut f, 0xc3, &sof);
        seg(&mut f, 0xda, &[1, 1, 0x00, predictor, 0, 0]);
        f.extend(entropy);
        f.extend([0xff, 0xd9]);
        f
    }

    /// A lossless JPEG of three components, the first sampled `h × v`, with
    /// predictor 1 and the 5-bit table of [`encode`], coded as Canon's small
    /// RAWs are ([`Header::lines`]): `lines[c][y]` is component `c`'s line
    /// `y`, `n_c` samples per MCU, each sample predicted from its left
    /// neighbour on that line (the first from the line above, the very first
    /// from half the range).
    pub(crate) fn encode_lines(lines: &[Vec<Vec<u16>>; 3], h: u8, v: u8, precision: u8) -> Vec<u8> {
        let n = [usize::from(h) * usize::from(v), 1, 1];
        let rows = lines[0].len();
        let across = lines[0][0].len() / n[0];
        let mut bits: Vec<bool> = Vec::new();
        for y in 0..rows {
            for mx in 0..across {
                for c in 0..3 {
                    for j in 0..n[c] {
                        let x = mx * n[c] + j;
                        let at = |x: usize, y: usize| i32::from(lines[c][y][x]);
                        let p = match (y, x) {
                            (0, 0) => 1 << (precision - 1),
                            (_, 0) => at(0, y - 1),
                            _ => at(x - 1, y),
                        };
                        let d = at(x, y) - p;
                        let len = if d == 0 {
                            0
                        } else {
                            32 - d.unsigned_abs().leading_zeros()
                        };
                        for i in (0..5).rev() {
                            bits.push(len >> i & 1 == 1);
                        }
                        let code = if d < 0 { d + (1 << len) - 1 } else { d } as u32;
                        for i in (0..len).rev() {
                            bits.push(code >> i & 1 == 1);
                        }
                    }
                }
            }
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(true);
        }
        let mut f = vec![0xff, 0xd8];
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        let mut dht = vec![0x00];
        let mut counts = [0u8; 16];
        counts[4] = 17;
        dht.extend(counts);
        dht.extend(0..=16u8);
        seg(&mut f, 0xc4, &dht);
        let mut sof = vec![precision];
        sof.extend(((rows * usize::from(v)) as u16).to_be_bytes());
        sof.extend(((across * usize::from(h)) as u16).to_be_bytes());
        sof.extend([3, 1, h << 4 | v, 0, 2, 0x11, 0, 3, 0x11, 0]);
        seg(&mut f, 0xc3, &sof);
        seg(&mut f, 0xda, &[3, 1, 0x00, 2, 0x00, 3, 0x00, 1, 0, 0]);
        for byte in bits.chunks(8) {
            let b = byte.iter().fold(0u8, |a, &x| a << 1 | u8::from(x));
            f.push(b);
            if b == 0xff {
                f.push(0);
            }
        }
        f.extend([0xff, 0xd9]);
        f
    }

    #[test]
    fn decodes_subsampled_components_line_by_line() {
        // Two lines of three MCUs of 2 × 2 luma, Cb and Cr.
        let luma = vec![
            vec![100, 104, 98, 97, 120, 121, 119, 118, 140, 139, 150, 151],
            vec![90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101],
        ];
        let cb = vec![vec![16380, 16390, 16384], vec![16300, 16400, 16384]];
        let cr = vec![vec![16384, 16000, 16500], vec![16384, 16384, 16385]];
        let data = encode_lines(&[luma.clone(), cb.clone(), cr.clone()], 2, 2, 15);
        let mut header = super::header(&data).unwrap();
        header.lines = true;
        let mut out = Vec::new();
        decode_into(&header, &data, &AtomicBool::new(false), &mut out).unwrap();
        assert_eq!(header.mcus(), (3, 2));
        let mut want = Vec::new();
        for y in 0..2 {
            for mx in 0..3 {
                want.extend(&luma[y][mx * 4..mx * 4 + 4]);
                want.push(cb[y][mx]);
                want.push(cr[y][mx]);
            }
        }
        assert_eq!(out, want);
        for end in 0..data.len() {
            if let Ok(mut header) = super::header(&data[..end]) {
                header.lines = true;
                let _ = decode_into(&header, &data[..end], &AtomicBool::new(false), &mut out);
            }
        }
    }

    #[test]
    fn decodes_every_predictor() {
        let (w, h) = (13, 7);
        let samples: Vec<u16> = (0..w * h)
            .map(|i| ((i * 7919 + i * i * 31) % 4096) as u16)
            .collect();
        for predictor in 1..=7 {
            let data = encode(&samples, w, 12, predictor);
            let (header, out) = decode(&data, &AtomicBool::new(false)).unwrap();
            assert_eq!((header.width, header.height), (w as u16, h as u16));
            assert_eq!(out, samples, "predictor {predictor}");
        }
        let full: Vec<u16> = (0..w * h).map(|i| (i * 40503 % 65536) as u16).collect();
        let (_, out) = decode(&encode(&full, w, 16, 1), &AtomicBool::new(false)).unwrap();
        assert_eq!(out, full);
    }

    /// One component, `width` samples of 16 bits, coded by `codes` (symbol,
    /// code, its length, extra bits, their length) in order: a stream built
    /// bit by bit.
    fn stream(width: u16, table: &[(u8, u32, u32)], codes: &[(u32, u32)]) -> Vec<u8> {
        let mut bits = Vec::new();
        for &(v, n) in codes {
            for i in (0..n).rev() {
                bits.push(v >> i & 1 == 1);
            }
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(true);
        }
        let mut entropy = Vec::new();
        for byte in bits.chunks(8) {
            let b = byte.iter().fold(0u8, |a, &x| a << 1 | u8::from(x));
            entropy.push(b);
            if b == 0xff {
                entropy.push(0);
            }
        }
        let mut counts = [0u8; 16];
        let mut symbols = Vec::new();
        let mut sorted = table.to_vec();
        sorted.sort_by_key(|&(_, code, len)| (len, code));
        for &(symbol, _, len) in &sorted {
            counts[len as usize - 1] += 1;
            symbols.push(symbol);
        }
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        let mut f = vec![0xff, 0xd8];
        let mut dht = vec![0];
        dht.extend(counts);
        dht.extend(symbols);
        seg(&mut f, 0xc4, &dht);
        let mut sof = vec![16, 0, 1];
        sof.extend(width.to_be_bytes());
        sof.extend([1, 1, 0x11, 0]);
        seg(&mut f, 0xc3, &sof);
        seg(&mut f, 0xda, &[1, 1, 0, 1, 0, 0]);
        f.extend(entropy);
        f.extend([0xff, 0xd9]);
        f
    }

    #[test]
    fn sixteen_extra_bits_are_read_only_when_the_stream_says_so() {
        // Symbols: 16 → `0`, 0 → `10`, 1 → `11`.
        let table = [(16, 0, 1), (0, 0b10, 2), (1, 0b11, 2)];
        let cancel = AtomicBool::new(false);
        // As T.81 codes it: the first sample 32768 off the initial 32768, so 0
        // (wrapping); then 0, then +1.
        let spec = stream(3, &table, &[(0, 1), (0b10, 2), (0b11, 2), (1, 1)]);
        assert_eq!(decode(&spec, &cancel).unwrap().1, [0, 0, 1]);
        // The same samples written with 16 bits after the length 16 (0x8000,
        // -32768): T.81's reading would leave two bytes unread.
        let extra = stream(
            3,
            &table,
            &[(0, 1), (0x7fff, 16), (0b10, 2), (0b11, 2), (1, 1)],
        );
        assert_eq!(decode(&extra, &cancel).unwrap().1, [0, 0, 1]);
        // A stream both readings decode, the 16 bits read as its data: only
        // the reading that consumes the stream exactly counts.
        let ambiguous = stream(2, &table, &[(0, 1), (0x7fff, 16), (0b10, 2)]);
        assert_eq!(decode(&ambiguous, &cancel).unwrap().1, [0, 0]);
        // A stream whose 16-bit reading runs past its end: T.81's stands,
        // trailing data and all.
        let short = stream(2, &table, &[(0, 1), (0b10, 2), (0, 13)]);
        assert_eq!(decode(&short, &cancel).unwrap().1, [0, 0]);
    }

    #[test]
    fn a_point_transform_beyond_the_precision_is_an_error() {
        let mut data = encode(&[1, 2, 3, 4], 2, 12, 1);
        // The SOS's last byte (Ah, Al) before the entropy-coded data.
        let sos = data.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        data[sos + 9] = 15;
        assert!(decode(&data, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn truncated_and_garbage_streams_are_errors_or_zeros_not_panics() {
        let samples: Vec<u16> = (0..64).map(|i| i * 50).collect();
        let data = encode(&samples, 8, 12, 1);
        for end in 0..data.len() {
            let _ = decode(&data[..end], &AtomicBool::new(false));
        }
        assert!(decode(b"not a jpeg", &AtomicBool::new(false)).is_err());
    }

    /// A lossless JPEG of components sampled `(h, v)`, each a plane of
    /// `mcus × h` by `rows × v` samples, with predictor 1 and the same code
    /// table as [`encode`]; predictions as T.81 defines them (or, with
    /// `block_rows`, a block's first row predicting from the first row of the
    /// block above). Returns the stream and the samples in MCU order.
    pub(crate) fn encode_sampled(
        planes: &[Vec<u16>],
        sampling: &[(usize, usize)],
        mcus: usize,
        rows: usize,
        block_rows: bool,
    ) -> (Vec<u8>, Vec<u16>) {
        let precision = 16;
        let mut bits: Vec<bool> = Vec::new();
        let mut order = Vec::new();
        for my in 0..rows {
            for mx in 0..mcus {
                for (plane, &(h, v)) in planes.iter().zip(sampling) {
                    let at = |x: usize, y: usize| i32::from(plane[y * mcus * h + x]);
                    for i in 0..v {
                        for j in 0..h {
                            let (x, y) = (mx * h + j, my * v + i);
                            let p = match (x, y) {
                                (0, 0) => 1 << (precision - 1),
                                (_, 0) => at(x - 1, y),
                                (0, _) if block_rows && i == 0 => at(x, y - v),
                                (0, _) => at(x, y - 1),
                                _ => at(x - 1, y),
                            };
                            let d = (at(x, y) - p).rem_euclid(1 << precision);
                            let d = if d >= 1 << (precision - 1) {
                                d - (1 << precision)
                            } else {
                                d
                            };
                            let len = 32 - d.unsigned_abs().leading_zeros();
                            (0..5).rev().for_each(|k| bits.push(len >> k & 1 == 1));
                            let v = if d < 0 { d + (1 << len) - 1 } else { d } as u32;
                            (0..len).rev().for_each(|k| bits.push(v >> k & 1 == 1));
                            order.push(at(x, y) as u16);
                        }
                    }
                }
            }
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(true);
        }
        let mut entropy = Vec::new();
        for byte in bits.chunks(8) {
            let b = byte.iter().fold(0u8, |a, &x| a << 1 | u8::from(x));
            entropy.push(b);
            if b == 0xff {
                entropy.push(0);
            }
        }
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        let mut f = vec![0xff, 0xd8];
        let mut dht = vec![0x00];
        let mut counts = [0u8; 16];
        counts[4] = 17;
        dht.extend(counts);
        dht.extend(0..=16u8);
        seg(&mut f, 0xc4, &dht);
        let (hmax, vmax) = (sampling[0].0, sampling[0].1);
        let mut sof = vec![precision as u8];
        sof.extend(((rows * vmax) as u16).to_be_bytes());
        sof.extend(((mcus * hmax) as u16).to_be_bytes());
        sof.push(planes.len() as u8);
        let mut sos = vec![planes.len() as u8];
        for (k, &(h, v)) in sampling.iter().enumerate() {
            sof.extend([k as u8 + 1, (h << 4 | v) as u8, 0]);
            sos.extend([k as u8 + 1, 0]);
        }
        sos.extend([1, 0, 0]);
        seg(&mut f, 0xc3, &sof);
        seg(&mut f, 0xda, &sos);
        f.extend(entropy);
        f.extend([0xff, 0xd9]);
        (f, order)
    }

    #[test]
    fn sampled_components_predict_within_their_own_samples() {
        let plane = |w: usize, h: usize, seed: usize| -> Vec<u16> {
            (0..w * h)
                .map(|i| (1000 + (i * 7919 + seed) % 3000) as u16)
                .collect()
        };
        let (mcus, rows) = (5, 4);
        for (h, v) in [(2, 1), (2, 2)] {
            let planes = [
                plane(mcus * h, rows * v, 1),
                plane(mcus, rows, 2),
                plane(mcus, rows, 3),
            ];
            let sampling = [(h, v), (1, 1), (1, 1)];
            for block_rows in [false, true] {
                let (data, want) = encode_sampled(&planes, &sampling, mcus, rows, block_rows);
                let mut header = super::header(&data).unwrap();
                assert_eq!(header.mcus(), (mcus, rows));
                header.block_rows = block_rows;
                let mut out = Vec::new();
                decode_into(&header, &data, &AtomicBool::new(false), &mut out).unwrap();
                assert_eq!(out, want, "{h} × {v}, block rows {block_rows}");
            }
        }
    }
}
