//! Sony ARW and SR2 (and the Sony bodies Hasselblad sold under its name): the
//! raw image directory and how its data is coded, its size, colour filter and
//! levels; the maker notes are `vendor::sony`'s.
//!
//! Sources: TIFF 6 and TIFF/EP (directories, strips, tiles, CFAPattern,
//! YCbCr); ITU T.81 (lossless JPEG tiles); ExifTool's Sony tag
//! documentation (the raw directory's Sony tags); our own
//! measurements of how each coding's samples are laid out and of the levels
//! RAWmakase has always read, against LibRaw's output on our files.
use super::tiff::{Ifd, NAME_MAX, TiffFile};
use crate::codec::ljpeg;
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use crate::vendor::sony;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where an ARW's sensor data is and how it is coded.
#[derive(Clone, Debug, Default)]
pub(crate) struct ArwPlan {
    pub width: u32,
    pub height: u32,
    /// The data's strips or tiles, each (offset, length).
    pub chunks: Vec<(u64, u64)>,
    /// The tile size (tiled data only).
    pub tile: [u32; 2],
    /// ARW2's tone curve thresholds (SonyToneCurve).
    pub curve: [u16; 4],
    pub little: bool,
    /// Red, green, blue, second green.
    pub black: [u32; 4],
    pub white: u32,
    pub linear_white: [u32; 4],
}

/// The raw image directory: the full-resolution image among IFD0's SubIFDs
/// (or IFD0 itself on the oldest bodies), the largest that is not a JPEG
/// preview.
fn raw_ifd<'a>(t: &'a TiffFile) -> Option<&'a Ifd> {
    t.ifds
        .iter()
        .filter(|ifd| {
            t.int(ifd, 254).unwrap_or(0) & 1 == 0
                && matches!(t.int(ifd, 262), Some(32803 | 6 | 34892))
                && t.int(ifd, 259) != Some(6)
        })
        .max_by_key(|ifd| {
            u64::from(t.int(ifd, 256).unwrap_or(0)) * u64::from(t.int(ifd, 257).unwrap_or(0))
        })
}

/// Whether the TIFF `t` is a Sony RAW: Sony's make, or a Sony body under
/// another name with Sony's private directory.
pub(crate) fn is_sony(t: &TiffFile) -> bool {
    let ifd0 = &t.ifds[0];
    let make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    let make = make.trim().to_ascii_uppercase();
    make.starts_with("SONY") || (make.starts_with("HASSELBLAD") && sony::has_private(t))
}

/// Reads a Sony RAW's header facts. `t` is the parsed TIFF.
pub(crate) fn open(t: TiffFile) -> Result<RawInfo> {
    let Some(raw) = raw_ifd(&t) else {
        return a100(&t);
    };
    let width = t.int(raw, 256).ok_or_else(|| corrupt("ARW width"))?;
    let height = t.int(raw, 257).ok_or_else(|| corrupt("ARW height"))?;
    crate::io::samples(width, height, 1)?;
    let bps = t.int(raw, 258).unwrap_or(12);
    let compression = t.int(raw, 259).unwrap_or(1);
    let photometric = t.int(raw, 262).unwrap_or(0);
    let (tile, chunks) = chunks(&t, raw, width, height)?;
    let stored: u64 = chunks.iter().map(|c| c.1).sum();
    let pixels = u64::from(width) * u64::from(height);
    let variant = match (compression, photometric) {
        (7, 6) => Variant::ArwYcbcr,
        (7, _) => Variant::ArwLosslessJpeg,
        (1, _) => Variant::ArwUncompressed,
        (32767, _) if stored == pixels => Variant::ArwCurve,
        (32767, _) if bps == 12 && width % 2 == 0 && stored == pixels * 3 / 2 => Variant::ArwPacked,
        (32767, _) => Variant::ArwV1,
        (32766, _) => Variant::ArwV6,
        _ => {
            return Err(Error::Unsupported(format!("ARW compression {compression}")));
        }
    };
    // ARW1's columns hold eight rows more than the directory says.
    let height = if variant == Variant::ArwV1 {
        height + 8
    } else {
        height
    };
    let mut info = header(&t, variant, [width, height], &chunks);
    if variant != Variant::ArwYcbcr {
        info.cfa = Some(cfa(&t, raw));
    } else {
        info.sraw = true;
        info.balanced = true;
    }
    let curve = t
        .ints(raw, 0x7010, 4)
        .filter(|c| c.len() == 4)
        .map(|c| std::array::from_fn(|i| c[i].min(0xffff) as u16))
        .unwrap_or([8000, 10400, 12900, 14100]);
    let levels = sony::read(&t, raw, &mut info);
    // Levels are 16-bit values: larger ones are corrupt (and would overflow
    // the scaling below).
    let fits = |v: u32| v <= 0xffff;
    if !levels.black.is_none_or(|b| b.iter().all(|&v| fits(v))) || !levels.white.is_none_or(fits) {
        return Err(corrupt("ARW black or white level"));
    }
    // Without levels in the file: 12-bit packed data's, else 14 bits'.
    let (black, white) = match variant {
        Variant::ArwPacked => (128, 4095),
        Variant::ArwV1 => (0, 4095),
        _ => (512, 16383),
    };
    // Packed 12-bit data's black is recorded on the 14-bit scale.
    let scale = if variant == Variant::ArwPacked { 2 } else { 0 };
    let mut black = levels.black.map_or([black; 4], |b| b.map(|v| v >> scale));
    let mut white = levels.white.unwrap_or(white);
    if variant == Variant::ArwYcbcr {
        // Small RAWs' RGB sits 512 above the sensor's black (our
        // measurements); its white is the 14-bit scale's, Adobe DNG
        // Converter's WhiteLevel for every small RAW of our dataset.
        black = black.map(|b| b + 512);
        white = 16383;
    }
    if variant == Variant::ArwV6 {
        // Compressed RAW 2's values are on twice the 14-bit scale; its white
        // is the curve's top.
        black = black.map(|b| 2 * b);
        white = u32::from(crate::codec::arw6::curve()[4095]);
    }
    info.plan = Plan::Arw(ArwPlan {
        width,
        height,
        chunks,
        tile,
        curve,
        // The DSC-R1 (SR2) stores its words big-endian in a little-endian
        // file (our measurements).
        little: t.little() && info.model.trim() != "DSC-R1",
        black,
        white,
        linear_white: levels.linear_white,
    });
    Ok(info)
}

/// The facts every Sony RAW reads alike: names, orientation, size,
/// exposure, previews (not the sensor data in `chunks`).
fn header(
    t: &TiffFile,
    variant: Variant,
    [width, height]: [u32; 2],
    chunks: &[(u64, u64)],
) -> RawInfo {
    let mut info = crate::info::base(variant);
    let ifd0 = &t.ifds[0];
    info.make = t.text_upto(ifd0, 271, NAME_MAX).unwrap_or_default();
    info.model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    info.raw_width = width;
    info.raw_height = height;
    info.width = width;
    info.height = height;
    super::tiff_exif::read(t, &mut info);
    // The previews, not the small RAWs' JPEG-coded sensor data.
    info.previews = t.previews();
    info.previews
        .retain(|p| !chunks.iter().any(|&(at, _)| at == p.offset));
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    info
}

/// The DSLR-A100: no raw directory, but IFD0's SubIFDs tag holds where its
/// ARW1 data starts, and Minolta's MRW blocks (in SR2Private) its size and
/// white balance. Its colour filter starts with green, then red.
fn a100(t: &TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let model = t.text_upto(ifd0, 272, NAME_MAX).unwrap_or_default();
    // Without a mosaic: the SRF's four-colour sensor, ARQ's pixel-shift
    // composites of four colours per pixel.
    let unsupported = || {
        Error::Unsupported(format!(
            "Sony RAW without a three-colour mosaic ({}): four colours or composites",
            model.trim()
        ))
    };
    if model.trim() != "DSLR-A100" {
        return Err(unsupported());
    }
    let (Some(at), Some(mrw)) = (t.int(ifd0, 330), sony::Mrw::read(t)) else {
        return Err(unsupported());
    };
    // One column more than the sensor's recorded width (our measurements).
    let [width, height] = [mrw.width + 1, mrw.height];
    crate::io::samples(width, height, 1)?;
    let at = t.base + u64::from(at);
    let chunks = vec![(at, (t.bytes.data.len() as u64).saturating_sub(at))];
    let mut info = header(t, Variant::ArwV1, [width, height], &chunks);
    info.cfa = Some(crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: vec![1, 0, 2, 1],
    }));
    info.cam_mul = mrw.cam_mul;
    info.presets = mrw.presets;
    info.plan = Plan::Arw(ArwPlan {
        width,
        height,
        chunks,
        tile: [width, height],
        curve: [0; 4],
        little: t.little(),
        black: [0; 4],
        white: 4095,
        linear_white: [0; 4],
    });
    Ok(info)
}

/// The strips or tiles of the raw image, and the tile size.
/// Each strip's or tile's (offset, length) in the file.
type Chunks = Vec<(u64, u64)>;

fn chunks(t: &TiffFile, ifd: &Ifd, width: u32, height: u32) -> Result<([u32; 2], Chunks)> {
    let (offsets, counts, tile) = match (t.int(ifd, 322), t.int(ifd, 323)) {
        (Some(tw), Some(th)) => {
            if tw == 0 || th == 0 {
                return Err(corrupt("ARW tiles"));
            }
            (t.pieces(ifd, 324), t.pieces(ifd, 325), [tw, th])
        }
        _ => (t.pieces(ifd, 273), t.pieces(ifd, 279), [width, height]),
    };
    let (Some(offsets), Some(counts)) = (offsets, counts) else {
        return Err(corrupt("ARW without data offsets"));
    };
    if offsets.len() != counts.len() || offsets.is_empty() {
        return Err(corrupt("ARW data offsets"));
    }
    let chunks = offsets
        .iter()
        .zip(&counts)
        .map(|(&o, &c)| (t.base + u64::from(o), u64::from(c)))
        .collect();
    Ok((tile, chunks))
}

/// The colour filter from CFAPattern (red, green, blue), red first when the
/// file has none; the green sharing a row with blue is the second green.
fn cfa(t: &TiffFile, ifd: &Ifd) -> Cfa {
    let pattern = t
        .bytes_of(ifd, 33422)
        .filter(|p| p.len() == 4 && p.iter().all(|&c| c <= 2))
        .map_or(vec![0, 1, 1, 2], <[u8]>::to_vec);
    crate::info::second_green(Cfa {
        rows: 2,
        cols: 2,
        colors: pattern,
    })
}

/// The bits of the scale a variant's values are on, which the camera
/// table's rows of `bits` are for: compressed RAW 2's are on twice the
/// 14-bit scale.
fn scale_bits(variant: Variant) -> u32 {
    match variant {
        Variant::ArwPacked | Variant::ArwV1 => 12,
        Variant::ArwV6 => 15,
        _ => 14,
    }
}

/// Decodes the sensor data.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &ArwPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let file = Bytes::new(
        bytes,
        if plan.little {
            Order::Little
        } else {
            Order::Big
        },
    );
    // Small RAWs are red, green, blue and an empty fourth channel.
    let channels = if info.variant == Variant::ArwYcbcr {
        4
    } else {
        1
    };
    let mut data = vec![0u16; crate::io::samples(plan.width, plan.height, channels)?];
    let strip = |i: usize| -> Result<&[u8]> {
        let (offset, len) = *plan.chunks.get(i).ok_or_else(|| corrupt("ARW data"))?;
        // Truncated files keep what they have.
        let len = len.min((bytes.len() as u64).saturating_sub(offset));
        file.slice(offset, len)
    };
    match info.variant {
        Variant::ArwCurve => {
            crate::codec::sony::arw2(strip(0)?, width, height, &mut data, cancel)?;
            let curve = crate::codec::sony::tone_curve(plan.curve);
            for v in &mut data {
                *v = curve[usize::from(*v) * 2];
            }
        }
        Variant::ArwUncompressed => {
            let src = strip(0)?;
            for (y, (row, dst)) in src
                .chunks(width * 2)
                .zip(data.chunks_exact_mut(width))
                .enumerate()
            {
                if y % 256 == 0 && cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                for (d, c) in dst.iter_mut().zip(row.as_chunks::<2>().0) {
                    *d = if plan.little {
                        u16::from_le_bytes(*c)
                    } else {
                        u16::from_be_bytes(*c)
                    };
                }
            }
        }
        Variant::ArwPacked => {
            let src = strip(0)?;
            for (y, (row, dst)) in src
                .chunks(width / 2 * 3)
                .zip(data.chunks_exact_mut(width))
                .enumerate()
            {
                if y % 256 == 0 && cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                crate::codec::sony::packed12(row, dst);
            }
        }
        Variant::ArwLosslessJpeg => ljpeg_tiles(plan, &strip, &mut data, cancel)?,
        Variant::ArwV6 => {
            crate::codec::arw6::decode(strip(0)?, width, height, &mut data, cancel)?;
            let curve = crate::codec::arw6::curve();
            for v in &mut data {
                *v = curve[usize::from(*v).min(4095)];
            }
        }
        Variant::ArwV1 => crate::codec::sony::arw1(strip(0)?, width, height, &mut data, cancel)?,
        Variant::ArwYcbcr => ycbcr_tiles(plan, &strip, &mut data, cancel)?,
        v => {
            return Err(Error::Unsupported(format!(
                "{} is not an ARW variant",
                v.name()
            )));
        }
    }
    let [left, top] = info.margins;
    let size = [info.raw_width, info.raw_height];
    // A white for the file's ISO speed before the camera's.
    let white = crate::facts::iso_white(&info.make, &info.model, false, size, info.iso)
        .or_else(|| {
            crate::facts::camera(&info.make, &info.model, false, size)
                .filter(|c| c.bits.is_none_or(|b| b == scale_bits(info.variant)))
                .find_map(|c| c.white)
        })
        .unwrap_or(plan.white);
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels,
        data,
        area: [left, top, info.width, info.height],
        black: plan.black,
        black_pattern: (0, 0, Vec::new()),
        white,
        linear_white: plan.linear_white,
    })
}

/// Small RAWs' lossless JPEG tiles of luma (sampled 2 × 1 or 2 × 2) and two
/// chroma samples per MCU, as red, green, blue and an empty fourth channel.
fn ycbcr_tiles<'a>(
    plan: &ArwPlan,
    strip: &impl Fn(usize) -> Result<&'a [u8]>,
    rgb: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let [tw, th] = plan.tile.map(|v| v as usize);
    let across = width.div_ceil(tw);
    let tiles = across * height.div_ceil(th);
    if plan.chunks.len() < tiles {
        return Err(corrupt("ARW tile count"));
    }
    let mut samples = Vec::new();
    for i in 0..tiles {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let tile = strip(i)?;
        let mut h = ljpeg::header(tile)?;
        h.block_rows = true;
        let [luma, cb, cr] = h.components[..] else {
            return Err(Error::Unsupported("ARW YCbCr tile components".into()));
        };
        let (bw, bh) = (usize::from(luma.h), usize::from(luma.v));
        if [cb.h, cb.v, cr.h, cr.v] != [1; 4] || bw > 2 || bh > 2 {
            return Err(Error::Unsupported("ARW YCbCr sampling".into()));
        }
        ljpeg::decode_into(&h, tile, cancel, &mut samples)?;
        let (mcus, _) = h.mcus();
        let per = h.samples_per_mcu();
        let (x0, y0) = ((i % across) * tw, (i / across) * th);
        for ty in 0..th.min(usize::from(h.height)) {
            let y = y0 + ty;
            if y >= height {
                break;
            }
            for tx in 0..tw.min(usize::from(h.width)) {
                let x = x0 + tx;
                if x >= width {
                    break;
                }
                let mcu = &samples[((ty / bh) * mcus + tx / bw) * per..][..per];
                let luma = mcu[(ty % bh) * bw + tx % bw];
                let [r, g, b] = crate::codec::sony::ycbcr_rgb(luma, mcu[bw * bh], mcu[bw * bh + 1]);
                rgb[(y * width + x) * 4..][..3].copy_from_slice(&[r, g, b]);
            }
        }
    }
    Ok(())
}

/// Lossless JPEG tiles: each a frame half the tile's width and height whose
/// four components are the tile's 2 × 2 colour cells (top left, top right,
/// bottom left, bottom right).
fn ljpeg_tiles<'a>(
    plan: &ArwPlan,
    strip: &impl Fn(usize) -> Result<&'a [u8]>,
    data: &mut [u16],
    cancel: &AtomicBool,
) -> Result<()> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let [tw, th] = plan.tile.map(|v| v as usize);
    let across = width.div_ceil(tw);
    let tiles = across * height.div_ceil(th);
    if plan.chunks.len() < tiles {
        return Err(corrupt("ARW tile count"));
    }
    let mut samples = Vec::new();
    for i in 0..tiles {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let tile = strip(i)?;
        let h = ljpeg::header(tile)?;
        if h.components.len() != 4 || h.samples_per_mcu() != 4 {
            return Err(Error::Unsupported(
                "ARW lossless JPEG tile without four components".into(),
            ));
        }
        ljpeg::decode_into(&h, tile, cancel, &mut samples)?;
        let (fw, fh) = (usize::from(h.width), usize::from(h.height));
        let (x0, y0) = ((i % across) * tw, (i / across) * th);
        for fy in 0..fh.min(th / 2) {
            for k in 0..4 {
                let y = y0 + 2 * fy + (k >> 1);
                if y >= height {
                    continue;
                }
                let row = &mut data[y * width..(y + 1) * width];
                for fx in 0..fw.min(tw / 2) {
                    let x = x0 + 2 * fx + (k & 1);
                    if x < width {
                        row[x] = samples[(fy * fw + fx) * 4 + k];
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tag's value in a test TIFF: numbers, text, bytes of a type, or the
    /// offset of a directory or a blob (a LONG).
    pub(crate) enum Value {
        Short(Vec<u16>),
        Long(Vec<u32>),
        Ascii(&'static str),
        Bytes(u16, Vec<u8>),
        Ifd(usize),
        Blob(usize),
        /// A directory's offset as four bytes (DNGPrivateData).
        IfdBytes(usize),
    }

    fn encoded(v: &Value) -> (u16, u32, Vec<u8>) {
        match v {
            Value::Short(s) => (
                3,
                s.len() as u32,
                s.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ),
            Value::Long(l) => (
                4,
                l.len() as u32,
                l.iter().flat_map(|v| v.to_le_bytes()).collect(),
            ),
            Value::Ascii(t) => {
                let mut b = t.as_bytes().to_vec();
                b.push(0);
                (2, b.len() as u32, b)
            }
            Value::Bytes(kind, b) => (*kind, b.len() as u32, b.clone()),
            Value::Ifd(_) | Value::Blob(_) => (4, 1, vec![0; 4]),
            Value::IfdBytes(_) => (1, 4, vec![0; 4]),
        }
    }

    /// A little-endian TIFF: directory 0 is IFD0, the others reached through
    /// `Value::Ifd`, then the blobs.
    pub(crate) fn tiff(ifds: Vec<Vec<(u16, Value)>>, blobs: Vec<Vec<u8>>) -> Vec<u8> {
        tiff_at(ifds, blobs).0
    }

    /// [`tiff`], and where each blob is in it.
    pub(crate) fn tiff_at(
        ifds: Vec<Vec<(u16, Value)>>,
        blobs: Vec<Vec<u8>>,
    ) -> (Vec<u8>, Vec<usize>) {
        let padded = |n: usize| if n > 4 { n + n % 2 } else { 0 };
        let mut at = 8;
        let mut ifd_at = vec![];
        for ifd in &ifds {
            ifd_at.push(at);
            at += 6 + 12 * ifd.len();
            at += ifd
                .iter()
                .map(|(_, v)| padded(encoded(v).2.len()))
                .sum::<usize>();
        }
        let mut blob_at = vec![];
        for b in &blobs {
            blob_at.push(at);
            at += b.len();
        }
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        for (k, ifd) in ifds.iter().enumerate() {
            let mut sorted: Vec<_> = ifd.iter().collect();
            sorted.sort_by_key(|(t, _)| *t);
            out.extend((sorted.len() as u16).to_le_bytes());
            let mut extra = ifd_at[k] + 6 + 12 * sorted.len();
            let mut values = Vec::new();
            for (tag, v) in sorted {
                let (kind, count, mut bytes) = encoded(v);
                match v {
                    Value::Ifd(i) => bytes = (ifd_at[*i] as u32).to_le_bytes().to_vec(),
                    Value::Blob(i) => bytes = (blob_at[*i] as u32).to_le_bytes().to_vec(),
                    Value::IfdBytes(i) => bytes = (ifd_at[*i] as u32).to_le_bytes().to_vec(),
                    _ => {}
                }
                out.extend(tag.to_le_bytes());
                out.extend(kind.to_le_bytes());
                out.extend(count.to_le_bytes());
                if bytes.len() <= 4 {
                    bytes.resize(4, 0);
                    out.extend(&bytes);
                } else {
                    out.extend((extra as u32).to_le_bytes());
                    extra += padded(bytes.len());
                    bytes.resize(padded(bytes.len()), 0);
                    values.extend(bytes);
                }
            }
            out.extend(0u32.to_le_bytes());
            out.extend(values);
        }
        for b in &blobs {
            out.extend(b);
        }
        (out, blob_at)
    }

    /// A Sony ARW of `width × height` whose raw directory has `compression`
    /// and `bps`, its data `strip`, and `extra` tags.
    pub(crate) fn arw(
        model: &'static str,
        [width, height]: [u32; 2],
        [bps, compression]: [u16; 2],
        extra: Vec<(u16, Value)>,
        strip: Vec<u8>,
    ) -> Vec<u8> {
        let mut raw = vec![
            (254, Value::Long(vec![0])),
            (256, Value::Long(vec![width])),
            (257, Value::Long(vec![height])),
            (258, Value::Short(vec![bps])),
            (259, Value::Short(vec![compression])),
            (262, Value::Short(vec![32803])),
            (273, Value::Blob(0)),
            (279, Value::Long(vec![strip.len() as u32])),
            (33421, Value::Short(vec![2, 2])),
            (33422, Value::Bytes(1, vec![0, 1, 1, 2])),
        ];
        raw.extend(extra);
        tiff(
            vec![
                vec![
                    (271, Value::Ascii("SONY")),
                    (272, Value::Ascii(model)),
                    (274, Value::Short(vec![6])),
                    (330, Value::Ifd(1)),
                ],
                raw,
            ],
            vec![strip],
        )
    }

    fn decode(file: &[u8]) -> Result<(RawInfo, Sensor)> {
        let info = crate::open(file)?;
        let sensor = crate::decode(&info, file, &AtomicBool::new(false))?;
        Ok((info, sensor))
    }

    #[test]
    fn uncompressed_arws_read_their_words_and_sony_tags() {
        let (w, h) = (6, 4);
        let samples: Vec<u16> = (0..w * h).map(|i| 500 + i as u16 * 37).collect();
        let strip = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let file = arw(
            "ILCE-7M3",
            [w, h],
            [14, 1],
            vec![
                (0x7310, Value::Short(vec![510, 511, 512, 513])),
                (0x7313, Value::Short(vec![2048, 1024, 1024, 1536])),
                (50717, Value::Short(vec![16300])),
                (0x74c7, Value::Short(vec![2, 0])),
                (0x74c8, Value::Short(vec![4, 4])),
            ],
            strip,
        );
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::ArwUncompressed);
        assert_eq!(
            (info.make.as_str(), info.model.as_str()),
            ("Sony", "ILCE-7M3")
        );
        assert_eq!(info.flip, 6);
        assert_eq!(info.frame, [2, 0, 4, 4]);
        assert_eq!(info.cam_mul, [2048., 1024., 1536., 1024.]);
        assert_eq!(s.data, samples);
        // Red, green, green, blue as red, green, blue, second green.
        assert_eq!(s.black, [510, 511, 513, 512]);
        assert_eq!(s.white, 16300);
        assert_eq!(info.cfa.unwrap().colors, vec![0, 1, 3, 2]);
    }

    #[test]
    fn levels_beyond_16_bits_are_corrupt() {
        // Black and white as LONGs of u32::MAX, in every variant.
        for (compression, bytes) in [(1u16, 32 * 16 * 2), (32767, 32 * 16)] {
            for tag in [0x7310u16, 50717] {
                let n = if tag == 0x7310 { 4 } else { 1 };
                let file = arw(
                    "ILCE-7M3",
                    [32, 16],
                    [14, compression],
                    vec![(tag, Value::Long(vec![u32::MAX; n]))],
                    vec![0; bytes],
                );
                assert!(
                    matches!(crate::open(&file), Err(Error::Corrupt(_))),
                    "compression {compression}, tag {tag:#x}"
                );
            }
        }
    }

    #[test]
    fn small_raw_levels_beyond_16_bits_are_corrupt() {
        // A small RAW (photometric YCbCr) adds 512 to its black.
        let mut file = arw(
            "ILCE-7RM5",
            [32, 16],
            [15, 7],
            vec![(0x7310, Value::Long(vec![u32::MAX; 4]))],
            vec![0; 64],
        );
        let at = file
            .windows(8)
            .position(|w| w == [6, 1, 3, 0, 1, 0, 0, 0])
            .unwrap();
        file[at + 8..at + 10].copy_from_slice(&6u16.to_le_bytes());
        assert!(matches!(crate::open(&file), Err(Error::Corrupt(_))));
    }

    #[test]
    fn compressed_raw_2_levels_beyond_16_bits_are_corrupt() {
        // Compressed RAW 2 doubles its black.
        let file = arw(
            "ILCE-7M5",
            [32, 16],
            [14, 32766],
            vec![(0x7310, Value::Long(vec![u32::MAX; 4]))],
            vec![0; 64],
        );
        assert!(matches!(crate::open(&file), Err(Error::Corrupt(_))));
    }

    #[test]
    fn arw2_rows_go_through_the_tone_curve() {
        // One row of 32 pixels: two blocks of 16, maximum first, minimum
        // second, steps of 3 between.
        let block = |max: u16, min: u16| {
            let mut word = u128::from(max) | u128::from(min) << 11 | 1 << 26;
            for i in 0..14 {
                word |= 3u128 << (30 + 7 * i);
            }
            word.to_le_bytes()
        };
        let mut strip = block(1100, 1000).to_vec();
        strip.extend(block(1050, 1001));
        let file = arw(
            "NEX-5N",
            [32, 1],
            [12, 32767],
            vec![(0x7010, Value::Short(vec![8000, 10400, 12900, 14100]))],
            strip,
        );
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::ArwCurve);
        // A code is the point twice it on the curve: slope 1 to 2000 (code
        // 1000), then 2.
        let point = |c: u32| {
            if c <= 1000 {
                2 * c
            } else {
                2000 + 4 * (c - 1000)
            }
        };
        assert_eq!(u32::from(s.data[0]), point(1100));
        assert_eq!(u32::from(s.data[2]), point(1000));
        assert_eq!(u32::from(s.data[4]), point(1003));
        assert_eq!(u32::from(s.data[1]), point(1050));
        assert_eq!(u32::from(s.data[3]), point(1001));
        // Adobe's white for the NEX-5N.
        assert_eq!((s.black, s.white), ([512; 4], 16300));
    }

    #[test]
    fn white_balance_and_levels_come_from_the_encrypted_sr2subifd() {
        // The SR2SubIFD: a directory whose value offsets are file offsets,
        // encrypted with the key in SR2Private.
        let block = |start: usize| {
            let mut b = vec![];
            b.extend(2u16.to_le_bytes());
            for (tag, at) in [(0x7310u16, 30), (0x7313, 38)] {
                b.extend(tag.to_le_bytes());
                b.extend(3u16.to_le_bytes());
                b.extend(4u32.to_le_bytes());
                b.extend(((start + at) as u32).to_le_bytes());
            }
            b.extend([0; 4]);
            for v in [800u16, 800, 800, 800, 2320, 1024, 1024, 1884] {
                b.extend(v.to_le_bytes());
            }
            sony::decrypt(&mut b, 0x4433_2211);
            b
        };
        let file = |start: usize| {
            let samples: Vec<u8> = vec![0; 2 * 4 * 2];
            let raw = vec![
                (254, Value::Long(vec![0])),
                (256, Value::Long(vec![4])),
                (257, Value::Long(vec![2])),
                (258, Value::Short(vec![14])),
                (259, Value::Short(vec![1])),
                (262, Value::Short(vec![32803])),
                (273, Value::Blob(0)),
                (279, Value::Long(vec![16])),
            ];
            let private = vec![
                (0x7200, Value::Blob(1)),
                (0x7201, Value::Long(vec![46])),
                (0x7221, Value::Bytes(1, vec![0x11, 0x22, 0x33, 0x44])),
            ];
            tiff_at(
                vec![
                    vec![
                        (271, Value::Ascii("SONY")),
                        (272, Value::Ascii("DSC-RX100M6")),
                        (330, Value::Ifd(1)),
                        (0xc634, Value::IfdBytes(2)),
                    ],
                    raw,
                    private,
                ],
                vec![samples, block(start)],
            )
        };
        // The block's place, then the file with the block written for it.
        let (_, at) = file(0);
        let (file, _) = file(at[1]);
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.cam_mul, [2320., 1024., 1884., 1024.]);
        assert_eq!(s.black, [800; 4]);
    }

    #[test]
    fn lossless_jpeg_tiles_hold_two_by_two_cells() {
        // One 4 × 4 tile: a 2 × 2 frame of four components, the cells' top
        // left, top right, bottom left, bottom right.
        let image: Vec<u16> = (0..16).map(|i| 1000 + i * 111).collect();
        let cell = |k: usize| -> Vec<u16> {
            (0..4)
                .map(|f| image[(2 * (f / 2) + (k >> 1)) * 4 + 2 * (f % 2) + (k & 1)])
                .collect()
        };
        let planes: Vec<Vec<u16>> = (0..4).map(cell).collect();
        let (tile, _) =
            crate::codec::ljpeg::tests::encode_sampled(&planes, &[(1, 1); 4], 2, 2, false);
        let file = arw(
            "ILCE-7M4",
            [4, 4],
            [14, 7],
            vec![
                (322, Value::Long(vec![4])),
                (323, Value::Long(vec![4])),
                (324, Value::Blob(0)),
                (325, Value::Long(vec![tile.len() as u32])),
            ],
            tile,
        );
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::ArwLosslessJpeg);
        assert_eq!(s.data, image);
    }

    #[test]
    fn small_raws_are_luma_and_chroma_tiles() {
        // A 4 × 2 tile of luma sampled 2 × 2, so two MCUs.
        let luma: Vec<u16> = vec![1000, 1010, 1020, 1030, 1040, 1050, 1060, 1070];
        let (cb, cr) = (vec![16383 + 10, 16383], vec![16383 - 20, 16383]);
        let (tile, _) = crate::codec::ljpeg::tests::encode_sampled(
            &[luma.clone(), cb, cr],
            &[(2, 2), (1, 1), (1, 1)],
            2,
            1,
            true,
        );
        let mut file = arw(
            "ILCE-7RM5",
            [4, 2],
            [15, 7],
            vec![
                (322, Value::Long(vec![4])),
                (323, Value::Long(vec![2])),
                (324, Value::Blob(0)),
                (325, Value::Long(vec![tile.len() as u32])),
                (0x7310, Value::Short(vec![512; 4])),
            ],
            tile,
        );
        // Photometric YCbCr.
        let at = file
            .windows(8)
            .position(|w| w == [6, 1, 3, 0, 1, 0, 0, 0])
            .unwrap();
        file[at + 8..at + 10].copy_from_slice(&6u16.to_le_bytes());
        let (info, s) = decode(&file).unwrap();
        assert_eq!(info.variant, Variant::ArwYcbcr);
        assert!(info.sraw && info.balanced && info.cfa.is_none());
        assert_eq!((s.channels, s.black, s.white), (4, [1024; 4], 16383));
        let pixel = |x: usize, y: usize| &s.data[(y * 4 + x) * 4..][..4];
        let [r, g, b] = crate::codec::sony::ycbcr_rgb(1050, 16393, 16363);
        assert_eq!(pixel(1, 1), [r, g, b, 0]);
        assert_eq!(pixel(2, 0), [1020, 1020, 1020, 0]);
    }

    #[test]
    fn truncated_and_garbage_files_are_errors_not_panics() {
        let samples: Vec<u8> = (0..64u8).collect();
        let files = [
            arw("ILCE-7M3", [4, 4], [14, 1], vec![], samples.clone()),
            arw(
                "NEX-5N",
                [32, 1],
                [12, 32767],
                vec![],
                samples[..32].to_vec(),
            ),
            arw("DSLR-A200", [8, 4], [12, 32767], vec![], samples.clone()),
            arw(
                "DSLR-A900",
                [4, 4],
                [12, 32767],
                vec![],
                samples[..24].to_vec(),
            ),
            arw("ILCE-7M4", [4, 4], [14, 7], vec![], samples.clone()),
        ];
        for file in files {
            for end in (0..file.len()).step_by(3) {
                let _ = decode(&file[..end]);
            }
            let mut noisy = file.clone();
            for (i, b) in noisy.iter_mut().enumerate().skip(8) {
                *b ^= (i * 7919 % 251) as u8;
            }
            let _ = decode(&noisy);
        }
    }
}
