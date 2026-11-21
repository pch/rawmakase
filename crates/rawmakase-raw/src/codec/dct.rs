//! DCT JPEG (ITU T.81, Huffman coded, 8 or 12-bit samples): sequential
//! (baseline and extended) and progressive. It decodes two kinds of DNG tile:
//! one component, as some writers (Blackmagic) store raw tiles under the
//! lossless JPEG compression code, and three, as DNG's lossy JPEG
//! (compression 34892) stores linear raw. Adobe DNG Converter codes all three
//! at full resolution in one sequential scan; Lightroom Classic's exports
//! subsample the chroma (4:2:2) or code progressively. The output is the
//! frame's samples in raster order, a pixel's components side by side.
//!
//! The DNG SDK reads these tiles with libjpeg, so what libjpeg does past the
//! standard is followed: three components coded as YCbCr are converted to RGB
//! in its fixed-point arithmetic, and subsampled chroma is upsampled by its
//! "fancy" (triangle) filters, as libjpeg-turbo computes them. The inverse
//! DCT is computed exactly (in double precision) and rounded, where libjpeg's
//! is in fixed point.
use super::bits::Msb;
use super::huffman::{Huffman, extend};
use crate::error::{Error, Result, corrupt};
use rawmakase_cancel::Cancel;

/// Whether `data` is a DCT JPEG this module decodes (its first frame is
/// SOF0, SOF1 or SOF2).
pub(crate) fn is_dct(data: &[u8]) -> bool {
    frame_marker(data).is_some_and(|m| matches!(m, 0xc0..=0xc2))
}

fn frame_marker(data: &[u8]) -> Option<u8> {
    frame_segment(data).map(|(m, _)| m)
}

/// The size the first frame header of `data` declares, read before anything
/// is decoded, so that a caller can refuse a frame other than it expects
/// before its planes are allocated.
pub(crate) fn frame_size(data: &[u8]) -> Option<Frame> {
    let (_, body) = frame_segment(data)?;
    let b = body.get(..6)?;
    Some(Frame {
        width: usize::from(u16::from_be_bytes([b[3], b[4]])),
        height: usize::from(u16::from_be_bytes([b[1], b[2]])),
        components: usize::from(b[5]),
    })
}

/// The marker and body of the first frame header of `data`.
fn frame_segment(data: &[u8]) -> Option<(u8, &[u8])> {
    let mut at = 2;
    while at + 4 <= data.len() && data[at] == 0xff {
        let m = data[at + 1];
        let len = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        if (0xc0..=0xcf).contains(&m) && m != 0xc4 && m != 0xc8 && m != 0xcc {
            return Some((m, data.get(at + 4..at + 2 + len)?));
        }
        at += 2 + len;
    }
    None
}

/// The natural position of each zig-zag index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// The size of a decoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// The samples of a pixel: 1 or 3.
    pub(crate) components: usize,
}

/// A frame component.
struct Component {
    id: u8,
    /// Its horizontal and vertical sampling factors.
    h: usize,
    v: usize,
    quant: usize,
    /// Its samples a row and its rows: the frame's, divided by the ratio of
    /// the largest sampling factor to its own.
    width: usize,
    height: usize,
    /// The blocks a row and block rows of its plane: whole MCUs.
    blocks_x: usize,
    blocks_y: usize,
}

/// The frame header.
struct Header {
    precision: u8,
    width: usize,
    height: usize,
    progressive: bool,
    components: Vec<Component>,
    /// The largest sampling factors: an MCU's size in blocks.
    h_max: usize,
    v_max: usize,
}

impl Header {
    fn read(body: &[u8], progressive: bool) -> Result<Self> {
        let n = usize::from(*body.get(5).ok_or_else(|| corrupt("SOF"))?);
        if !matches!(n, 1 | 3) {
            return Err(Error::Unsupported(format!("DCT JPEG of {n} components")));
        }
        let specs = body.get(6..6 + 3 * n).ok_or_else(|| corrupt("SOF"))?;
        let precision = body[0];
        let height = usize::from(u16::from_be_bytes([body[1], body[2]]));
        let width = usize::from(u16::from_be_bytes([body[3], body[4]]));
        if !matches!(precision, 8 | 12) {
            return Err(Error::Unsupported("DCT JPEG precision".into()));
        }
        let factors = |c: &[u8]| (usize::from(c[1] >> 4), usize::from(c[1] & 15));
        if specs
            .chunks(3)
            .any(|c| !(1..=4).contains(&factors(c).0) || !(1..=4).contains(&factors(c).1))
        {
            return Err(corrupt("DCT JPEG sampling factors"));
        }
        let h_max = specs.chunks(3).map(|c| factors(c).0).max().unwrap_or(1);
        let v_max = specs.chunks(3).map(|c| factors(c).1).max().unwrap_or(1);
        // A single component is coded alone, whatever its factors. Of
        // several, each is at full resolution or half across (4:2:2), or
        // half both ways (4:2:0): the upsampling libjpeg filters.
        if n > 1
            && specs.chunks(3).any(|c| {
                let (h, v) = factors(c);
                !matches!((h_max / h, v_max / v), (1, 1) | (2, 1) | (2, 2))
                    || h_max % h != 0
                    || v_max % v != 0
            })
        {
            return Err(Error::Unsupported("DCT JPEG sampling".into()));
        }
        crate::io::samples(width as u32, height as u32, n as u32)?;
        let (mcus_x, mcus_y) = (width.div_ceil(8 * h_max), height.div_ceil(8 * v_max));
        let components = specs
            .chunks(3)
            .map(|c| {
                let (h, v) = factors(c);
                Component {
                    id: c[0],
                    h,
                    v,
                    quant: usize::from(c[2] & 3),
                    width: (width * h).div_ceil(h_max),
                    height: (height * v).div_ceil(v_max),
                    blocks_x: mcus_x * h,
                    blocks_y: mcus_y * v,
                }
            })
            .collect();
        Ok(Self {
            precision,
            width,
            height,
            progressive,
            components,
            h_max,
            v_max,
        })
    }
}

/// The tables a scan codes by, as the headers before it defined them.
struct Tables {
    quant: [Option<[u16; 64]>; 4],
    dc: [Option<Huffman>; 4],
    ac: [Option<Huffman>; 4],
}

/// A scan: its components (indices into the frame's, with their DC and AC
/// table identifiers), spectral selection and successive approximation.
struct Scan {
    components: Vec<(usize, usize, usize)>,
    start: usize,
    end: usize,
    high: u32,
    low: u32,
}

impl Scan {
    fn read(body: &[u8], frame: &Header) -> Result<Self> {
        let n = usize::from(*body.first().ok_or_else(|| corrupt("SOS"))?);
        let rest = body
            .get(1..1 + 2 * n + 3)
            .filter(|_| (1..=frame.components.len()).contains(&n))
            .ok_or_else(|| corrupt("SOS"))?;
        let mut components = Vec::with_capacity(n);
        for s in rest[..2 * n].chunks(2) {
            let c = frame
                .components
                .iter()
                .position(|c| c.id == s[0])
                .filter(|c| !components.iter().any(|&(d, _, _)| d == *c))
                .ok_or_else(|| corrupt("SOS component"))?;
            components.push((c, usize::from(s[1] >> 4), usize::from(s[1] & 15)));
        }
        let [start, end, a] = [rest[2 * n], rest[2 * n + 1], rest[2 * n + 2]];
        let scan = Self {
            components,
            start: usize::from(start),
            end: usize::from(end),
            high: u32::from(a >> 4),
            low: u32::from(a & 15),
        };
        // A progressive scan codes DC alone (of any components) or a band of
        // AC of one component; its bits are shifted by at most 13, as
        // libjpeg allows.
        if frame.progressive
            && (scan.end > 63
                || scan.start > scan.end
                || (scan.start == 0) != (scan.end == 0)
                || (scan.start > 0 && n != 1)
                || scan.low > 13)
        {
            return Err(corrupt("progressive JPEG scan"));
        }
        Ok(scan)
    }
}

/// What a scan carries from block to block, reset at each restart marker:
/// each component's DC prediction, and the blocks left of an end-of-band
/// run.
#[derive(Default)]
struct State {
    predictions: [i32; 4],
    end_of_band: u32,
}

/// Decodes `data` into `out`: `height` rows of `width` pixels of
/// `components` samples each.
pub(crate) fn decode_into(data: &[u8], cancel: Cancel<'_>, out: &mut Vec<u16>) -> Result<Frame> {
    if data.len() < 4 || data[..2] != [0xff, 0xd8] {
        return Err(corrupt("DCT JPEG without SOI"));
    }
    let mut tables = Tables {
        quant: [None; 4],
        dc: [None, None, None, None],
        ac: [None, None, None, None],
    };
    let mut frame: Option<Header> = None;
    // Each component's samples (sequential) or coefficients (progressive),
    // whole blocks.
    let mut planes: Vec<Vec<u16>> = Vec::new();
    let mut coefficients: Vec<Vec<[i16; 64]>> = Vec::new();
    let mut restart = 0usize;
    // The markers libjpeg reads a three-component frame's color space by.
    let (mut jfif, mut adobe) = (false, None);
    let mut scans = 0;
    let mut at = 2;
    loop {
        if at + 2 > data.len() || data[at] != 0xff || data[at + 1] == 0xd9 {
            // The end of the image, or of the data after a scan.
            if scans > 0 {
                break;
            }
            return Err(corrupt("DCT JPEG headers end early"));
        }
        let marker = data[at + 1];
        if marker == 0xff {
            at += 1;
            continue;
        }
        let len = data
            .get(at + 2..at + 4)
            .map_or(0, |l| usize::from(u16::from_be_bytes([l[0], l[1]])));
        let Some(body) = data.get(at + 4..at + 2 + len).filter(|_| len >= 2) else {
            // Damaged data after a scan: the image as decoded so far, as a
            // truncated scan reads zeros.
            if scans > 0 {
                break;
            }
            return Err(corrupt("DCT JPEG segment"));
        };
        match marker {
            0xdb => {
                let mut b = body;
                while let Some(&pq) = b.first() {
                    let (wide, id) = (pq >> 4 == 1, usize::from(pq & 15));
                    let size = if wide { 128 } else { 64 };
                    let t = b.get(1..1 + size).ok_or_else(|| corrupt("DQT"))?;
                    let mut table = [0u16; 64];
                    for (k, z) in ZIGZAG.iter().enumerate() {
                        table[*z] = if wide {
                            u16::from_be_bytes([t[2 * k], t[2 * k + 1]])
                        } else {
                            u16::from(t[k])
                        };
                    }
                    *tables.quant.get_mut(id).ok_or_else(|| corrupt("DQT id"))? = Some(table);
                    b = &b[1 + size..];
                }
            }
            0xc4 => {
                let mut b = body;
                while b.len() >= 17 {
                    let (class, id) = (b[0] >> 4, usize::from(b[0] & 15));
                    let counts: [u8; 16] = b[1..17].try_into().map_err(|_| corrupt("DHT"))?;
                    let n: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    let symbols = b.get(17..17 + n).ok_or_else(|| corrupt("DHT"))?;
                    let table = Some(Huffman::new(&counts, symbols)?);
                    match (class, id) {
                        (0, 0..=3) => tables.dc[id] = table,
                        (1, 0..=3) => tables.ac[id] = table,
                        _ => return Err(corrupt("DHT class")),
                    }
                    b = &b[17 + n..];
                }
            }
            0xc0..=0xc2 => {
                if frame.is_some() {
                    return Err(corrupt("DCT JPEG of two frames"));
                }
                let f = Header::read(body, marker == 0xc2)?;
                for c in &f.components {
                    let blocks = c.blocks_x * c.blocks_y;
                    if f.progressive {
                        coefficients.push(vec![[0; 64]; blocks]);
                    } else {
                        planes.push(vec![0; blocks * 64]);
                    }
                }
                frame = Some(f);
            }
            0xdd => {
                let [a, b] = <[u8; 2]>::try_from(body).map_err(|_| corrupt("DRI"))?;
                restart = usize::from(u16::from_be_bytes([a, b]));
            }
            0xe0 => jfif |= body.starts_with(b"JFIF\0"),
            0xee if body.len() >= 12 && body.starts_with(b"Adobe") => adobe = Some(body[11]),
            0xda => {
                let f = frame.as_ref().ok_or_else(|| corrupt("SOS before SOF"))?;
                let scan = Scan::read(body, f)?;
                let start = at + 2 + len;
                let end = entropy_end(data, start);
                let mut bits = Msb::jpeg(&data[start..end]);
                if f.progressive {
                    progressive_scan(
                        f,
                        &scan,
                        &tables,
                        restart,
                        &mut bits,
                        cancel,
                        &mut coefficients,
                    )?;
                } else {
                    sequential_scan(f, &scan, &tables, restart, &mut bits, cancel, &mut planes)?;
                }
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                scans += 1;
                at = end;
                continue;
            }
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err(Error::Unsupported(format!("JPEG process {marker:#x}")));
            }
            _ => {}
        }
        at += 2 + len;
    }
    let f = frame.ok_or_else(|| corrupt("SOS before SOF"))?;
    if f.progressive {
        let basis = basis();
        for (c, coefficients) in f.components.iter().zip(&coefficients) {
            let quant =
                tables.quant[c.quant].ok_or_else(|| corrupt("missing quantization table"))?;
            let mut plane = vec![0; c.blocks_x * c.blocks_y * 64];
            for (i, block) in coefficients.iter().enumerate() {
                if i % c.blocks_x == 0 && cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                let block = block.map(i32::from);
                let (x, y) = (i % c.blocks_x, i / c.blocks_x);
                idct(
                    &block,
                    &quant,
                    &basis,
                    f.precision,
                    &mut plane,
                    c.blocks_x,
                    x,
                    y,
                );
            }
            planes.push(plane);
        }
    }
    let n = f.components.len();
    out.clear();
    out.resize(f.width * f.height * n, 0);
    for (i, (c, plane)) in f.components.iter().zip(&planes).enumerate() {
        upsample(&f, c, plane, i, out);
    }
    let max = (1i32 << f.precision) - 1;
    if n == 3 && ycbcr(jfif, adobe, &f.components) {
        let center = 1i32 << (f.precision - 1);
        for pixel in out.as_chunks_mut::<3>().0 {
            ycbcr_to_rgb(pixel, center, max);
        }
    }
    Ok(Frame {
        width: f.width,
        height: f.height,
        components: n,
    })
}

/// Where the entropy-coded data from `start` ends: at the first marker but a
/// restart marker.
fn entropy_end(data: &[u8], start: usize) -> usize {
    let mut i = start;
    while i + 1 < data.len() {
        if data[i] == 0xff && data[i + 1] != 0 && !(0xd0..=0xd7).contains(&data[i + 1]) {
            return i;
        }
        i += 1;
    }
    data.len()
}

/// Calls `block` for each block a scan codes, in its order, with the scan
/// component's index and the block's position in the component's plane;
/// `state` is reset at each restart marker. A scan of one component codes
/// its blocks one by one, over its own size; a scan of several codes MCUs,
/// each component's `h` × `v` blocks in turn.
fn each_block(
    frame: &Header,
    scan: &Scan,
    restart: usize,
    bits: &mut Msb,
    cancel: Cancel<'_>,
    mut block: impl FnMut(&mut Msb, &mut State, usize, usize, usize) -> Result<()>,
) -> Result<()> {
    let mut state = State::default();
    let (units_x, units_y) = match scan.components[..] {
        [(c, _, _)] => {
            let c = &frame.components[c];
            (c.width.div_ceil(8), c.height.div_ceil(8))
        }
        _ => (
            frame.width.div_ceil(8 * frame.h_max),
            frame.height.div_ceil(8 * frame.v_max),
        ),
    };
    let mut left = restart;
    for y in 0..units_y {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        for x in 0..units_x {
            if restart > 0 {
                if left == 0 {
                    bits.restart();
                    state = State::default();
                    left = restart;
                }
                left -= 1;
            }
            if let [(_, _, _)] = scan.components[..] {
                block(bits, &mut state, 0, x, y)?;
                continue;
            }
            for (s, &(c, _, _)) in scan.components.iter().enumerate() {
                let c = &frame.components[c];
                for v in 0..c.v {
                    for h in 0..c.h {
                        block(bits, &mut state, s, x * c.h + h, y * c.v + v)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// The Huffman table `id` of `tables`.
fn table<'a>(tables: &'a [Option<Huffman>; 4], id: usize, what: &str) -> Result<&'a Huffman> {
    tables
        .get(id)
        .and_then(Option::as_ref)
        .ok_or_else(|| corrupt(what))
}

/// Decodes a sequential scan, each block into its component's plane.
fn sequential_scan(
    frame: &Header,
    scan: &Scan,
    tables: &Tables,
    restart: usize,
    bits: &mut Msb,
    cancel: Cancel<'_>,
    planes: &mut [Vec<u16>],
) -> Result<()> {
    let basis = basis();
    // Each scan component: its frame component, quantizer, DC and AC tables.
    let coding = scan
        .components
        .iter()
        .map(|&(c, d, a)| {
            let quant = tables.quant[frame.components[c].quant]
                .ok_or_else(|| corrupt("missing quantization table"))?;
            let dc = table(&tables.dc, d, "DC table")?;
            Ok((c, quant, dc, table(&tables.ac, a, "AC table")?))
        })
        .collect::<Result<Vec<_>>>()?;
    each_block(
        frame,
        scan,
        restart,
        bits,
        cancel,
        |bits, state, s, x, y| {
            let (c, ref quant, dc, ac) = coding[s];
            let mut block = [0i32; 64];
            let size = u32::from(dc.symbol(bits)?);
            if size > 16 {
                return Err(corrupt("DC difference"));
            }
            let prediction = &mut state.predictions[s];
            *prediction = prediction.wrapping_add(extend(bits.get(size), size));
            block[0] = *prediction;
            let mut k = 1;
            while k < 64 {
                let rs = ac.symbol(bits)?;
                let (run, size) = (usize::from(rs >> 4), u32::from(rs & 15));
                if size == 0 {
                    if run == 15 {
                        k += 16;
                        continue;
                    }
                    break;
                }
                k += run;
                if k >= 64 {
                    return Err(corrupt("AC run past the block"));
                }
                block[ZIGZAG[k]] = extend(bits.get(size), size);
                k += 1;
            }
            let blocks_x = frame.components[c].blocks_x;
            idct(
                &block,
                quant,
                &basis,
                frame.precision,
                &mut planes[c],
                blocks_x,
                x,
                y,
            );
            Ok(())
        },
    )
}

/// Decodes a progressive scan into its components' coefficients: the first
/// bits of DC or of a band of AC, or one more bit of them (T.81, G.1.2).
fn progressive_scan(
    frame: &Header,
    scan: &Scan,
    tables: &Tables,
    restart: usize,
    bits: &mut Msb,
    cancel: Cancel<'_>,
    coefficients: &mut [Vec<[i16; 64]>],
) -> Result<()> {
    let (band, low) = (scan.start..=scan.end, scan.low);
    let tables = scan
        .components
        .iter()
        .map(|&(c, d, a)| {
            Ok(match (scan.start, scan.high) {
                (0, 0) => (c, Some(table(&tables.dc, d, "DC table")?)),
                (0, _) => (c, None),
                _ => (c, Some(table(&tables.ac, a, "AC table")?)),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // The value of a coefficient's bit at `low`, positive and negative.
    let (plus, minus) = (1i16 << low, -1i16 << low);
    // One more bit of a coefficient already non-zero: its magnitude's.
    let refine = |bits: &mut Msb, v: &mut i16| {
        if bits.get(1) == 1 && *v & plus == 0 {
            *v = v.wrapping_add(if *v >= 0 { plus } else { minus });
        }
    };
    each_block(
        frame,
        scan,
        restart,
        bits,
        cancel,
        |bits, state, s, x, y| {
            let (c, table) = tables[s];
            let blocks_x = frame.components[c].blocks_x;
            let block = &mut coefficients[c][y * blocks_x + x];
            match (scan.start, scan.high, table) {
                (0, 0, Some(dc)) => {
                    let size = u32::from(dc.symbol(bits)?);
                    if size > 16 {
                        return Err(corrupt("DC difference"));
                    }
                    let prediction = &mut state.predictions[s];
                    *prediction = prediction.wrapping_add(extend(bits.get(size), size));
                    block[0] = prediction.wrapping_shl(low) as i16;
                }
                (0, _, _) => {
                    if bits.get(1) == 1 {
                        block[0] |= plus;
                    }
                }
                (_, 0, Some(ac)) => {
                    if state.end_of_band > 0 {
                        state.end_of_band -= 1;
                        return Ok(());
                    }
                    let mut k = *band.start();
                    while k <= *band.end() {
                        let rs = ac.symbol(bits)?;
                        let (run, size) = (u32::from(rs >> 4), u32::from(rs & 15));
                        if size == 0 {
                            if run < 15 {
                                state.end_of_band = (1 << run) - 1 + bits.get(run);
                                break;
                            }
                            k += 16;
                            continue;
                        }
                        k += run as usize;
                        if k > *band.end() {
                            return Err(corrupt("AC run past the band"));
                        }
                        block[ZIGZAG[k]] = extend(bits.get(size), size).wrapping_shl(low) as i16;
                        k += 1;
                    }
                }
                (_, _, Some(ac)) => {
                    // As libjpeg refines: each code places a new coefficient of
                    // magnitude 1 after `run` zeros, refining the non-zero ones it
                    // passes; an end-of-band run refines the rest of its blocks.
                    let mut k = *band.start();
                    if state.end_of_band == 0 {
                        while k <= *band.end() {
                            let rs = ac.symbol(bits)?;
                            let (mut run, size) = (u32::from(rs >> 4), rs & 15);
                            let mut value = 0;
                            if size == 0 {
                                if run < 15 {
                                    state.end_of_band = (1 << run) + bits.get(run);
                                    break;
                                }
                            } else if size == 1 {
                                value = if bits.get(1) == 1 { plus } else { minus };
                            } else {
                                return Err(corrupt("AC refinement size"));
                            }
                            while k <= *band.end() {
                                let z = &mut block[ZIGZAG[k]];
                                if *z != 0 {
                                    refine(bits, z);
                                } else if run == 0 {
                                    break;
                                } else {
                                    run -= 1;
                                }
                                k += 1;
                            }
                            if value != 0 {
                                *block
                                    .get_mut(ZIGZAG.get(k).copied().unwrap_or(64))
                                    .filter(|_| k <= *band.end())
                                    .ok_or_else(|| corrupt("AC run past the band"))? = value;
                            }
                            k += 1;
                        }
                    }
                    if state.end_of_band > 0 {
                        for &z in &ZIGZAG[k.min(64)..=*band.end()] {
                            if block[z] != 0 {
                                refine(bits, &mut block[z]);
                            }
                        }
                        state.end_of_band -= 1;
                    }
                }
                _ => unreachable!("a DC first or AC scan has its table"),
            }
            Ok(())
        },
    )
}

/// Writes the samples of `block` (coefficients in natural order, not yet
/// multiplied by `quant`) to block (`x`, `y`) of `plane`, `blocks_x` blocks
/// a row.
#[allow(clippy::too_many_arguments)]
fn idct(
    block: &[i32; 64],
    quant: &[u16; 64],
    basis: &[[f64; 8]; 8],
    precision: u8,
    plane: &mut [u16],
    blocks_x: usize,
    x: usize,
    y: usize,
) {
    let max = f64::from((1u32 << precision) - 1);
    let center = f64::from(1u32 << (precision - 1));
    let f: [f64; 64] = std::array::from_fn(|i| f64::from(block[i]) * f64::from(quant[i]));
    // Samples: Σ basis(x,u) basis(y,v) F(v,u), rows first.
    let rows: [[f64; 8]; 8] = std::array::from_fn(|v| {
        std::array::from_fn(|i| (0..8).map(|u| basis[i][u] * f[v * 8 + u]).sum())
    });
    let stride = blocks_x * 8;
    for (j, weights) in basis.iter().enumerate() {
        let line = &mut plane[(y * 8 + j) * stride + x * 8..][..8];
        for (i, sample) in line.iter_mut().enumerate() {
            let s: f64 = (0..8).map(|v| weights[v] * rows[v][i]).sum();
            *sample = (s + center).round().clamp(0., max) as u16;
        }
    }
}

/// Writes component `index`'s samples from its `plane` to `out` at the
/// frame's size: as they are at full resolution; through libjpeg-turbo's
/// triangle filters where subsampled, each output sample ¾ of its nearest
/// input sample and ¼ of the next nearest, across (`h2v1`) or across and
/// down (`h2v2`), the edges repeated.
fn upsample(frame: &Header, c: &Component, plane: &[u16], index: usize, out: &mut [u16]) {
    let (width, n) = (frame.width, frame.components.len());
    let stride = c.blocks_x * 8;
    let row = |y: usize| &plane[y * stride..][..c.width];
    let (across, down) = (frame.h_max / c.h, frame.v_max / c.v);
    // libjpeg-turbo filters only components more than 2 samples wide, and
    // repeats the samples of narrower ones.
    let fancy = c.width > 2;
    let mut sums = vec![0u32; c.width];
    for y in 0..frame.height {
        let line = &mut out[y * width * n..][..width * n];
        let mut put = |x: usize, v: u32| {
            if x < width {
                line[x * n + index] = v as u16;
            }
        };
        match (across, down) {
            (2, _) if !fancy => {
                for (i, &v) in row(y / down).iter().enumerate() {
                    put(2 * i, u32::from(v));
                    put(2 * i + 1, u32::from(v));
                }
            }
            (2, 1) => {
                let input = row(y);
                let at = |i: usize| u32::from(input[i]);
                let last = c.width - 1;
                for i in 0..c.width {
                    let near = 3 * at(i);
                    let left = if i == 0 {
                        at(0)
                    } else {
                        (near + at(i - 1) + 1) >> 2
                    };
                    let right = if i == last {
                        at(last)
                    } else {
                        (near + at(i + 1) + 2) >> 2
                    };
                    put(2 * i, left);
                    put(2 * i + 1, right);
                }
            }
            (2, 2) => {
                // Each column's sum, ¾ of the nearest row and ¼ of the next
                // nearest, ×4; then the same across, ×16 in all.
                let r = y / 2;
                let other = if y % 2 == 0 {
                    r.saturating_sub(1)
                } else {
                    (r + 1).min(c.height - 1)
                };
                let (near, far) = (row(r), row(other));
                for (s, (&a, &b)) in sums.iter_mut().zip(near.iter().zip(far)) {
                    *s = 3 * u32::from(a) + u32::from(b);
                }
                let last = c.width - 1;
                for i in 0..c.width {
                    let near = 3 * sums[i];
                    let left = if i == 0 {
                        (4 * sums[0] + 8) >> 4
                    } else {
                        (near + sums[i - 1] + 8) >> 4
                    };
                    let right = if i == last {
                        (4 * sums[last] + 7) >> 4
                    } else {
                        (near + sums[i + 1] + 7) >> 4
                    };
                    put(2 * i, left);
                    put(2 * i + 1, right);
                }
            }
            _ => {
                for (x, &v) in row(y).iter().enumerate() {
                    put(x, u32::from(v));
                }
            }
        }
    }
}

/// Whether a three-component frame is YCbCr, by libjpeg's rules: a JFIF
/// marker says so; an Adobe marker's transform flag decides next (0 is RGB);
/// without either, components named `R`, `G` and `B` are RGB.
fn ycbcr(jfif: bool, adobe: Option<u8>, components: &[Component]) -> bool {
    if jfif {
        return true;
    }
    if let Some(transform) = adobe {
        return transform != 0;
    }
    components.iter().map(|c| c.id).ne(*b"RGB")
}

/// Converts a YCbCr pixel to RGB in libjpeg's fixed-point arithmetic (16
/// fractional bits, each term rounded as its tables round it), so that the
/// samples match the DNG SDK's.
fn ycbcr_to_rgb(pixel: &mut [u16; 3], center: i32, max: i32) {
    // FIX(1.40200), FIX(0.34414), FIX(0.71414) and FIX(1.77200).
    const CR_R: i32 = 91881;
    const CB_G: i32 = 22554;
    const CR_G: i32 = 46802;
    const CB_B: i32 = 116130;
    const HALF: i32 = 1 << 15;
    let y = i32::from(pixel[0]);
    let (cb, cr) = (i32::from(pixel[1]) - center, i32::from(pixel[2]) - center);
    let rgb = [
        y + ((CR_R * cr + HALF) >> 16),
        y + ((-CB_G * cb - CR_G * cr + HALF) >> 16),
        y + ((CB_B * cb + HALF) >> 16),
    ];
    for (s, v) in pixel.iter_mut().zip(rgb) {
        *s = v.clamp(0, max) as u16;
    }
}

/// The DCT basis: `basis[x][u]` = C(u)/2 · cos((2x + 1)uπ/16).
fn basis() -> [[f64; 8]; 8] {
    std::array::from_fn(|x| {
        std::array::from_fn(|u| {
            let c = if u == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.
            };
            c / 2. * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.).cos()
        })
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An 8 × 8 JPEG of three components named `ids`, under an Adobe
    /// marker of transform `adobe` if given: Y and Cr of DC 12 · 2 / 8 = 3
    /// above the level shift (131), Cb at it (128).
    pub(crate) fn three_component_jpeg(adobe: Option<u8>, ids: [u8; 3]) -> Vec<u8> {
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        let mut f = vec![0xff, 0xd8];
        if let Some(t) = adobe {
            seg(
                &mut f,
                0xee,
                &[b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, t],
            );
        }
        let mut dqt = vec![0];
        dqt.extend([2u8; 64]);
        seg(&mut f, 0xdb, &dqt);
        // DC: symbol 0 coded `0`, symbol 4 coded `10`; AC: EOB coded `0`.
        let mut dht = vec![0x00, 1, 1];
        dht.extend([0u8; 14]);
        dht.extend([0, 4]);
        dht.extend([0x10, 1]);
        dht.extend([0u8; 15]);
        dht.push(0);
        seg(&mut f, 0xc4, &dht);
        let [a, b, c] = ids;
        seg(
            &mut f,
            0xc0,
            &[8, 0, 8, 0, 8, 3, a, 0x11, 0, b, 0x11, 0, c, 0x11, 0],
        );
        seg(&mut f, 0xda, &[3, a, 0x00, b, 0x00, c, 0x00, 0, 63, 0]);
        // Y: `10` `1100` EOB; Cb: `0` EOB; Cr: `10` `1100` EOB.
        f.extend([0b1011_0000, 0b0101_1000]);
        f.extend([0xff, 0xd9]);
        f
    }

    /// A one-block 8-bit JPEG whose only coefficient is DC: every sample is
    /// the level shift plus DC / 8 · quantizer.
    #[test]
    fn a_flat_block_decodes_to_its_dc() {
        let mut f = vec![0xff, 0xd8];
        let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
            f.extend([0xff, m]);
            f.extend(((body.len() + 2) as u16).to_be_bytes());
            f.extend(body);
        };
        let mut dqt = vec![0];
        dqt.extend([2u8; 64]);
        seg(&mut f, 0xdb, &dqt);
        // DC: symbol 4 (a 4-bit difference) coded `0`; AC: EOB coded `0`.
        let mut dht = vec![0x00, 1];
        dht.extend([0u8; 15]);
        dht.push(4);
        dht.extend([0x10, 1]);
        dht.extend([0u8; 15]);
        dht.push(0);
        seg(&mut f, 0xc4, &dht);
        seg(&mut f, 0xc0, &[8, 0, 8, 0, 8, 1, 1, 0x11, 0]);
        seg(&mut f, 0xda, &[1, 1, 0x00, 0, 63, 0]);
        // DC difference 12 (`1100`), then EOB `0`: 0 1100 0, padded with ones.
        f.extend([0b0110_0011, 0xff, 0x00]);
        f.extend([0xff, 0xd9]);
        assert!(is_dct(&f));
        let mut out = Vec::new();
        let frame = decode_into(&f, Cancel::NEVER, &mut out).unwrap();
        assert_eq!(
            frame,
            Frame {
                width: 8,
                height: 8,
                components: 1
            }
        );
        // 12 · 2 / 8 = 3 above the level shift of 128.
        assert!(out.iter().all(|&v| v == 131), "{out:?}");
        for end in 0..f.len() {
            let _ = decode_into(&f[..end], Cancel::NEVER, &mut out);
        }
        // A restart interval segment too short to hold its value.
        let mut bad = f[..2].to_vec();
        bad.extend([0xff, 0xdd, 0, 2]);
        bad.extend(&f[2..]);
        assert!(decode_into(&bad, Cancel::NEVER, &mut out).is_err());
    }

    /// A one-block JPEG of three components, as DNG's lossy JPEG stores
    /// linear raw ([`three_component_jpeg`]). libjpeg reads it as YCbCr under an Adobe marker of transform 1,
    /// and as RGB under transform 0 or with components named R, G and B.
    #[test]
    fn three_components_decode_interleaved_and_ycbcr_converts_as_libjpeg_does() {
        let mut out = Vec::new();
        let mut decode = |f: &[u8]| {
            let frame = decode_into(f, Cancel::NEVER, &mut out).unwrap();
            assert_eq!((frame.width, frame.height, frame.components), (8, 8, 3));
            assert!(out.chunks(3).all(|p| p == &out[..3]), "{out:?}");
            out[..3].to_vec()
        };
        // R = 131 + round(1.402 · 3) = 135; G = 131 + ⌊−0.71414 · 3 + ½⌋
        // = 129; B = 131 + 0.
        assert_eq!(
            decode(&three_component_jpeg(Some(1), [0, 1, 2])),
            [135, 129, 131]
        );
        assert_eq!(
            decode(&three_component_jpeg(None, [1, 2, 3])),
            [135, 129, 131]
        );
        assert_eq!(
            decode(&three_component_jpeg(Some(0), [0, 1, 2])),
            [131, 128, 131]
        );
        assert_eq!(
            decode(&three_component_jpeg(None, *b"RGB")),
            [131, 128, 131]
        );
        let f = three_component_jpeg(Some(1), [0, 1, 2]);
        for end in 0..f.len() {
            let _ = decode_into(&f[..end], Cancel::NEVER, &mut out);
        }
        // Damaged data after the scan (a segment running past the end):
        // the image as decoded, not an error.
        let mut damaged = f[..f.len() - 2].to_vec();
        damaged.extend([0xff, 0xe1, 0x7f, 0xff, 0]);
        let frame = decode_into(&damaged, Cancel::NEVER, &mut out).unwrap();
        assert_eq!(frame.components, 3);
        assert_eq!(out[..3], [135, 129, 131]);
        // A scan naming a component twice.
        let mut twice = f.clone();
        let sos = twice.windows(2).position(|w| w == [0xff, 0xda]).unwrap();
        twice[sos + 7] = 0;
        assert!(decode_into(&twice, Cancel::NEVER, &mut out).is_err());
    }

    /// Synthetic 37 × 21 JPEGs written by libjpeg-turbo 3.1 (through
    /// Pillow, quality 92), with libjpeg-turbo's decodes of them as RGB
    /// (`.rgb`). Pixel (x, y) was red 230 or 20 in alternate 5-column bands,
    /// green (6x + 4y) mod 256 and blue 40 or 210 in alternate 3-row bands:
    /// sharp chroma edges, which upsampling other than libjpeg's would blur
    /// or shift by far more than an 8-bit step. `dct-422-restart` is
    /// sequential, its chroma half across (4:2:2), a restart marker every 3
    /// MCUs; `dct-420-progressive` is progressive, its chroma half both ways
    /// (4:2:0), with DC and AC refinement scans and a restart marker every 2
    /// MCUs. Both are JFIF, so YCbCr.
    #[test]
    fn subsampled_and_progressive_frames_decode_as_libjpeg_does() {
        let cases: [(&[u8], &[u8]); 2] = [
            (
                include_bytes!("../../tests/data/dct-422-restart.jpg"),
                include_bytes!("../../tests/data/dct-422-restart.rgb"),
            ),
            (
                include_bytes!("../../tests/data/dct-420-progressive.jpg"),
                include_bytes!("../../tests/data/dct-420-progressive.rgb"),
            ),
        ];
        let mut out = Vec::new();
        for (jpeg, libjpeg) in cases {
            let frame = decode_into(jpeg, Cancel::NEVER, &mut out).unwrap();
            assert_eq!((frame.width, frame.height, frame.components), (37, 21, 3));
            assert_eq!(out.len(), libjpeg.len());
            // libjpeg's inverse DCT is in fixed point, this one exact: a
            // sample may differ by a step or two, three after YCbCr.
            let off = |&(&a, &b): &(&u16, &u8)| a.abs_diff(u16::from(b));
            let pairs: Vec<_> = out.iter().zip(libjpeg).collect();
            assert!(pairs.iter().map(off).max() <= Some(3));
            let exact = pairs.iter().filter(|p| off(p) == 0).count();
            assert!(exact * 10 >= pairs.len() * 9, "{exact} of {}", pairs.len());
            for end in 0..jpeg.len() {
                let _ = decode_into(&jpeg[..end], Cancel::NEVER, &mut out);
            }
        }
    }

    /// A scan checks for cancellation as it goes, not only after it: a
    /// cancelled decode stops before reaching the corrupt code at its start.
    #[test]
    fn a_scan_stops_when_cancelled() {
        let mut f = three_component_jpeg(Some(1), [0, 1, 2]);
        // Entropy data of ones: `11` is no DC code.
        let at = f.len() - 4;
        f[at..at + 2].copy_from_slice(&[0xfe, 0xfe]);
        let mut out = Vec::new();
        assert!(matches!(
            decode_into(&f, Cancel::NEVER, &mut out),
            Err(Error::Corrupt(_))
        ));
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        assert!(matches!(
            decode_into(&f, Cancel::new(&cancelled), &mut out),
            Err(Error::Cancelled)
        ));
    }
}
