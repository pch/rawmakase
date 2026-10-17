//! Adobe DNG (DNG Specification 1.7): the raw image directory, its tiles or
//! strips and their compression, the CFA layout, levels and linearization, the
//! active area and default crop, the colour tags, and the data opcodes of
//! OpcodeList1 and 2 (`opcodes`).
use super::tiff::{Ifd, TiffFile};
use crate::codec::{bits::Msb, dct, ljpeg};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo, UNSET};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::{AtomicBool, Ordering};

mod opcodes;
use opcodes::Opcode;

/// Where a DNG's raw image is and how it is coded.
#[derive(Clone, Debug, Default)]
pub(crate) struct DngPlan {
    /// The levels: BlackLevelRepeatDim (rows, columns), BlackLevel,
    /// BlackLevelDeltaH and DeltaV (per column and row of the active area),
    /// and WhiteLevel.
    pub black_dim: Option<[u32; 2]>,
    pub black: Vec<f64>,
    pub black_delta_h: Vec<f64>,
    pub black_delta_v: Vec<f64>,
    pub white: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub bps: u32,
    pub compression: u32,
    /// SampleFormat: 1 unsigned integers, 3 floating point.
    pub sample_format: u32,
    /// Predictor: 1 none, 2 horizontal differences, 34894–34896 Adobe's
    /// floating-point predictor (bytes of 1, 2 or 4 samples at a time).
    pub predictor: u32,
    /// Tile (or strip) size; strips are tiles as wide as the image.
    pub tile: [u32; 2],
    /// Each tile's (offset, length).
    pub tiles: Tiles,
    pub little: bool,
    /// LinearizationTable: stored value to linear value.
    pub curve: Vec<u16>,
    /// OpcodeList1 and OpcodeList2's opcodes applied here.
    pub opcodes: [Vec<Opcode>; 2],
}

/// Reads a DNG's header facts. `t` is the parsed TIFF.
pub(crate) fn open(bytes: &[u8], t: TiffFile) -> Result<RawInfo> {
    let ifd0 = &t.ifds[0];
    let version = t
        .raw(ifd0, 50706)
        .filter(|v| v.len() >= 4)
        .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
        .ok_or(Error::NotRaw)?;
    // The raw image: the full-resolution directory (NewSubFileType 0) whose
    // photometric interpretation is a CFA or linear raw.
    let raw = (0..t.ifds.len())
        .find(|&i| {
            let ifd = &t.ifds[i];
            let full = t.int(ifd, 254).unwrap_or(0) & 1 == 0;
            let photometric = t.int(ifd, 262);
            full && matches!(photometric, Some(32803) | Some(34892))
        })
        .ok_or_else(|| Error::Unsupported("DNG without a raw image".into()))?;
    let ifd = &t.ifds[raw];
    let photometric = t.int(ifd, 262).unwrap_or(0);
    let samples = t.int(ifd, 277).unwrap_or(1);
    if photometric != 32803 || samples != 1 {
        return Err(Error::Unsupported("DNG linear raw".into()));
    }
    let width = t.int(ifd, 256).ok_or_else(|| corrupt("DNG width"))?;
    let height = t.int(ifd, 257).ok_or_else(|| corrupt("DNG height"))?;
    crate::io::samples(width, height, 1)?;
    let bps = t.int(ifd, 258).unwrap_or(1);
    let compression = t.int(ifd, 259).unwrap_or(1);
    let sample_format = t.int(ifd, 339).unwrap_or(1);
    let predictor = t.int(ifd, 317).unwrap_or(1);
    let (tile, tiles) = layout(&t, ifd, width, height)?;
    let variant = match (compression, sample_format) {
        (1, 1) => Variant::DngUncompressed,
        (7, 1) => Variant::DngLosslessJpeg,
        (8, _) => Variant::DngDeflate,
        (1, 3) => Variant::DngUncompressedFloat,
        _ => return Err(Error::Unsupported(format!("DNG compression {compression}"))),
    };
    let cfa = cfa(&t, ifd)?;
    let curve = t
        .ints(ifd, 50712)
        .map(|c| c.into_iter().map(|v| v.min(65535) as u16).collect())
        .unwrap_or_default();
    // Active area: top, left, bottom, right.
    let active = t
        .ints(ifd, 50829)
        .filter(|a| a.len() == 4 && a[2] > a[0] && a[3] > a[1] && a[2] <= height && a[3] <= width)
        .unwrap_or_else(|| vec![0, 0, height, width]);
    let (top, left) = (active[0], active[1]);
    let mut info = crate::info::base(variant);
    info.dng_version = version;
    info.raw_width = width;
    info.raw_height = height;
    info.margins = [left, top];
    info.width = active[3] - left;
    info.height = active[2] - top;
    info.cfa = Some(cfa.shifted(top as usize, left as usize));
    let origin = t.reals(ifd, 50719).filter(|v| v.len() == 2);
    let size = t.reals(ifd, 50720).filter(|v| v.len() == 2);
    // The default crop in sensor coordinates, starting on even rows and
    // columns (keeping its far edges); a crop of the whole image, or one
    // outside the sensor data, is none.
    let inside = |v: f64, max: u32| (v >= 0. && v <= f64::from(max)).then_some(v as u32);
    let crop = origin.zip(size).and_then(|(o, s)| {
        Some([
            left.checked_add(inside(o[0], width)?)?,
            top.checked_add(inside(o[1], height)?)?,
            inside(s[0], width)?,
            inside(s[1], height)?,
        ])
    });
    info.frame = match crop {
        Some([mut x, mut y, mut w, mut h]) => {
            if x % 2 == 1 {
                x += 1;
                w = w.saturating_sub(1);
            }
            if y % 2 == 1 {
                y += 1;
                h = h.saturating_sub(1);
            }
            if [x, y, w, h] == [0, 0, width, height] {
                [UNSET, UNSET, 0, 0]
            } else {
                [x, y, w, h]
            }
        }
        None => [UNSET, UNSET, 0, 0],
    };
    let ifd0 = &t.ifds[0];
    info.make = t.text(ifd0, 271).unwrap_or_default();
    info.model = t.text(ifd0, 272).unwrap_or_default();
    let unique = t.text(ifd0, 50708).filter(|u| !u.trim().is_empty());
    // Apple's DNGs name the camera module (UniqueCameraModel), not the phone.
    if info.make.trim() == "Apple"
        && let Some(unique) = &unique
    {
        info.model = unique.clone();
    }
    // Without Make and Model (Blackmagic's and Parrot's DNGs), the camera is
    // UniqueCameraModel's: the maker's name, then the model.
    if info.make.trim().is_empty()
        && info.model.trim().is_empty()
        && let Some(unique) = &unique
    {
        let unique = unique.trim();
        let (make, model) = unique.split_once(' ').unwrap_or(("", unique));
        info.make = make.to_string();
        info.model = model.trim().to_string();
    }
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    if let Some(neutral) = t.reals(ifd0, 50728).filter(|n| n.len() == 3) {
        for (mul, n) in info.cam_mul.iter_mut().zip(&neutral) {
            // In single precision, the neutral first: RAWmakase's white
            // balance to the last bit (our measurement on 130 DNGs).
            if *n > 0. {
                *mul = 1. / (*n as f32);
            }
        }
        info.cam_mul[3] = info.cam_mul[1];
    }
    let matrix = |tag| {
        t.reals(ifd0, tag)
            .filter(|m| m.len() == 9 && m.iter().all(|v| v.is_finite()))
    };
    info.file_matrix = d65_matrix(
        [matrix(50721), matrix(50722)],
        [t.int(ifd0, 50778), t.int(ifd0, 50779)],
    );
    super::tiff_exif::read(&t, &mut info);
    if let Some(notes) = crate::vendor::pentax::dng_notes(&t) {
        info.lens_id = notes.lens_id;
        if let Some(iso) = notes.iso {
            info.iso = iso;
        }
    } else if let Some(entry) = ifd0.entries.get(&50740)
        && let Some(at) = t.offset_of(entry)
    {
        // Nikon's maker note, in Adobe's record of DNGPrivateData.
        let model = crate::names::model("Nikon", &info.make, &info.model);
        if let Some(notes) = crate::vendor::nikon::read_dng(bytes, at, &model) {
            info.lens_id = notes.lens_id;
        }
    }
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let black_dim = t
        .ints(ifd, 50713)
        .filter(|d| d.len() == 2 && (1..=16).contains(&d[0]) && (1..=16).contains(&d[1]))
        .map(|d| [d[0], d[1]]);
    // Levels that are not numbers are none.
    let reals = |tag| -> Vec<f64> {
        t.reals(ifd, tag)
            .unwrap_or_default()
            .into_iter()
            .map(|v| if v.is_finite() { v } else { 0. })
            .collect()
    };
    let black = reals(50714);
    let black_delta_h = reals(50715);
    let black_delta_v = reals(50716);
    let white = t.int(ifd, 50717);
    info.plan = Plan::Dng(DngPlan {
        black_dim,
        black,
        black_delta_h,
        black_delta_v,
        white,
        width,
        height,
        bps,
        compression,
        sample_format,
        predictor,
        tile,
        tiles,
        little: t.little(),
        curve,
        opcodes: [list(&t, ifd, 51008), list(&t, ifd, 51009)],
    });
    Ok(info)
}

/// The colour temperature of an Exif LightSource code a calibration names
/// (Exif 2.3's illuminants: CIE standard illuminants A, B, C, D50–D75 and ISO
/// studio tungsten at their correlated temperatures, the others at the
/// middle of their ranges); `None` for unknown and other light.
fn illuminant_temperature(code: u32) -> Option<f64> {
    Some(match code {
        17 => 2856.,
        3 => 2850.,
        16 => 2940.,
        24 => 3200.,
        15 => 3450.,
        2 | 14 => 4150.,
        18 => 4874.,
        13 => 5000.,
        23 => 5003.,
        1 | 4 | 9 => 5500.,
        20 => 5503.,
        12 => 6430.,
        10 => 6500.,
        21 => 6504.,
        19 => 6774.,
        11 => 7500.,
        22 => 7504.,
        _ => return None,
    })
}

/// The XYZ-to-camera matrix at D65 from ColorMatrix1 and 2 (rows of three)
/// and their CalibrationIlluminants, as the DNG Specification's "Mapping
/// Camera Color Space to CIE XYZ Space" interpolates two calibrations: by
/// inverse colour temperature between them, the nearer one's beyond them.
/// D65 when a calibration is D65 (most files); one matrix alone is the
/// camera's; with an illuminant of unknown temperature, the first.
fn d65_matrix(
    matrices: [Option<Vec<f64>>; 2],
    illuminants: [Option<u32>; 2],
) -> Option<[[f32; 3]; 3]> {
    let to_f32 = |m: &[f64]| std::array::from_fn(|r| std::array::from_fn(|c| m[r * 3 + c] as f32));
    const D65: f64 = 6504.;
    let [m1, m2] = matrices;
    let (Some(m1), Some(m2)) = (&m1, &m2) else {
        return m1.or(m2).map(|m| to_f32(&m));
    };
    let temps = illuminants.map(|i| i.and_then(illuminant_temperature));
    let (Some(t1), Some(t2)) = (temps[0], temps[1]) else {
        return Some(to_f32(m1));
    };
    if t1 == D65 || t1 == t2 {
        return Some(to_f32(m1));
    }
    if t2 == D65 {
        return Some(to_f32(m2));
    }
    // The weight of the first calibration, in inverse temperature.
    let g = ((1. / D65 - 1. / t2) / (1. / t1 - 1. / t2)).clamp(0., 1.);
    let m: Vec<f64> = m1
        .iter()
        .zip(m2)
        .map(|(a, b)| g * a + (1. - g) * b)
        .collect();
    Some(to_f32(&m))
}

/// The opcodes of OpcodeList1 (`tag` 51008) or 2 (51009) applied to the
/// sensor data. OpcodeList2's FixVignetteRadial is not: development reads
/// it as the file's lens correction (rawmakase-model's DNG reader), with
/// OpcodeList3's warps.
fn list(t: &TiffFile, ifd: &Ifd, tag: u16) -> Vec<Opcode> {
    let ops = t
        .raw(ifd, tag)
        .and_then(|l| opcodes::parse(&l))
        .unwrap_or_default();
    ops.into_iter()
        .filter(|op| tag != 51009 || !matches!(op, Opcode::FixVignetteRadial { .. }))
        .collect()
}

/// The tile size and every tile's offset and length; strips as tiles.
/// Each tile's (offset, length) in the file.
type Tiles = Vec<(u64, u64)>;

fn layout(t: &TiffFile, ifd: &Ifd, width: u32, height: u32) -> Result<([u32; 2], Tiles)> {
    let base = t.base;
    if let (Some(tw), Some(th)) = (t.int(ifd, 322), t.int(ifd, 323)) {
        let offsets = t
            .ints(ifd, 324)
            .ok_or_else(|| corrupt("DNG tile offsets"))?;
        let counts = t.ints(ifd, 325).ok_or_else(|| corrupt("DNG tile counts"))?;
        // A tile is at most the image's size limit (and is decoded whole).
        crate::io::samples(tw, th, 1)?;
        if offsets.len() != counts.len() {
            return Err(corrupt("DNG tiles"));
        }
        let across = width.div_ceil(tw) as usize;
        let down = height.div_ceil(th) as usize;
        if offsets.len() < across * down {
            return Err(corrupt("DNG tile count"));
        }
        let tiles = offsets
            .iter()
            .zip(&counts)
            .map(|(&o, &c)| (base + u64::from(o), u64::from(c)))
            .collect();
        return Ok(([tw, th], tiles));
    }
    let offsets = t
        .ints(ifd, 273)
        .ok_or_else(|| corrupt("DNG strip offsets"))?;
    let counts = t
        .ints(ifd, 279)
        .ok_or_else(|| corrupt("DNG strip counts"))?;
    let rows = t.int(ifd, 278).unwrap_or(height).clamp(1, height);
    if offsets.len() != counts.len() || offsets.len() < height.div_ceil(rows) as usize {
        return Err(corrupt("DNG strips"));
    }
    let tiles = offsets
        .iter()
        .zip(&counts)
        .map(|(&o, &c)| (base + u64::from(o), u64::from(c)))
        .collect();
    Ok(([width, rows], tiles))
}

/// The colour filter over the raw image from CFARepeatPatternDim, CFAPattern
/// and CFAPlaneColor, in the visible area's coordinates once the active area's
/// offset is applied by the caller. The green sharing a row with blue is the
/// second green (3).
fn cfa(t: &TiffFile, ifd: &Ifd) -> Result<Cfa> {
    let dim = t
        .ints(ifd, 33421)
        .filter(|d| d.len() == 2)
        .unwrap_or(vec![2, 2]);
    let (rows, cols) = (dim[0] as usize, dim[1] as usize);
    let pattern = t
        .raw(ifd, 33422)
        .ok_or_else(|| corrupt("DNG CFA pattern"))?;
    if rows == 0 || cols == 0 || rows > 16 || cols > 16 || pattern.len() < rows * cols {
        return Err(corrupt("DNG CFA pattern"));
    }
    let planes = t.raw(ifd, 50710).unwrap_or(vec![0, 1, 2]);
    let mut colors = Vec::with_capacity(rows * cols);
    for &p in &pattern[..rows * cols] {
        let c = *planes
            .get(usize::from(p))
            .ok_or_else(|| corrupt("DNG CFA plane"))?;
        if c > 2 {
            return Err(Error::Unsupported("DNG with a four-colour CFA".into()));
        }
        colors.push(c);
    }
    Ok(crate::info::second_green(Cfa { rows, cols, colors }))
}

/// Decodes the raw image: every tile into the full stored image, then the
/// linearization table.
pub(crate) fn decode(
    info: &RawInfo,
    plan: &DngPlan,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Sensor> {
    let (width, height) = (plan.width as usize, plan.height as usize);
    let mut data = vec![0u16; crate::io::samples(plan.width, plan.height, 1)?];
    let [tw, th] = [plan.tile[0] as usize, plan.tile[1] as usize];
    let across = width.div_ceil(tw);
    let file = Bytes::new(
        bytes,
        if plan.little {
            Order::Little
        } else {
            Order::Big
        },
    );
    let mut decoded = Vec::new();
    for (i, &(offset, len)) in plan
        .tiles
        .iter()
        .enumerate()
        .take(across * height.div_ceil(th))
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let (tx, ty) = ((i % across) * tw, (i / across) * th);
        let tile = file.slice(offset, len)?;
        // The tile's samples in raster order, `tw` per row.
        let float = plan.sample_format == 3;
        let samples: &[u16] = match plan.compression {
            1 if float => {
                floats(tile, plan.bps, tw * th, plan.little, &mut decoded)?;
                &decoded
            }
            1 => {
                unpack(tile, plan.bps, tw, th, plan.little, &mut decoded)?;
                &decoded
            }
            8 => {
                let bytes_per = (plan.bps as usize).div_ceil(8);
                let limit = tw * th * bytes_per;
                let mut bytes =
                    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(tile, limit)
                        .map_err(|_| corrupt("DNG Deflate tile"))?;
                bytes.resize(limit, 0);
                if float && plan.predictor != 1 {
                    // The predictor leaves big-endian samples.
                    unpredict_float(&mut bytes, tw, bytes_per, plan.predictor)?;
                    floats(&bytes, plan.bps, tw * th, false, &mut decoded)?;
                } else if float {
                    floats(&bytes, plan.bps, tw * th, plan.little, &mut decoded)?;
                } else {
                    unpack(&bytes, plan.bps, tw, th, plan.little, &mut decoded)?;
                    if plan.predictor == 2 {
                        // Differences wrap at the samples' precision.
                        let mask = ((1u32 << plan.bps.clamp(1, 16)) - 1) as u16;
                        for row in decoded.chunks_mut(tw) {
                            for i in 1..row.len() {
                                row[i] = row[i].wrapping_add(row[i - 1]) & mask;
                            }
                        }
                    }
                }
                &decoded
            }
            // Lossless JPEG; some writers (Blackmagic) store DCT JPEG under
            // the same code.
            7 if dct::is_dct(tile) => {
                dct::decode_into(tile, cancel, &mut decoded)?;
                &decoded
            }
            7 => {
                ljpeg::decode_into(&ljpeg::header(tile)?, tile, cancel, &mut decoded)?;
                &decoded
            }
            c => return Err(Error::Unsupported(format!("DNG compression {c}"))),
        };
        for (k, &v) in samples.iter().enumerate() {
            let (y, x) = (ty + k / tw, tx + k % tw);
            if k / tw >= th {
                break;
            }
            if y < height && x < width {
                data[y * width + x] = v;
            }
        }
    }
    // OpcodeList1 works on the stored image.
    let mut stored = opcodes::Image {
        data: &mut data,
        stride: width,
        origin: [0, 0],
        width,
        height,
    };
    opcodes::apply(&plan.opcodes[0], &mut stored, cancel)?;
    if !plan.curve.is_empty() {
        let last = plan.curve.len() - 1;
        for v in &mut data {
            *v = plan.curve[usize::from(*v).min(last)];
        }
    }
    let [left, top] = info.margins;
    let area = [left, top, info.width, info.height];
    let white = plan
        .white
        .unwrap_or_else(|| ((1u64 << plan.bps.min(16)) - 1) as u32);
    if !whole_levels(plan) || !plan.opcodes[1].is_empty() {
        linearize(&mut data, plan, area, white, cancel)?;
        // OpcodeList2 works on the linear values of the active area.
        let mut active = opcodes::Image {
            data: &mut data,
            stride: width,
            origin: [left as usize, top as usize],
            width: info.width as usize,
            height: info.height as usize,
        };
        opcodes::apply(&plan.opcodes[1], &mut active, cancel)?;
        // The black pattern, applied, is zeros.
        let pattern = match plan.black_dim {
            Some([rows, cols]) => (rows, cols, vec![0; (rows * cols) as usize]),
            None => (0, 0, Vec::new()),
        };
        return Ok(Sensor {
            width: plan.width,
            height: plan.height,
            channels: 1,
            data,
            area,
            black: [0; 4],
            black_pattern: pattern,
            white: LINEAR_WHITE,
            linear_white: [0; 4],
        });
    }
    let (black, pattern) = crate::levels::dng_black(plan.black_dim, &plan.black);
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area,
        black,
        black_pattern: pattern,
        white,
        linear_white: [0; 4],
    })
}

/// The white level of linear reference values: 1.0 as 16-bit samples.
const LINEAR_WHITE: u32 = 65535;

/// Whether the levels are whole numbers the sensor data can keep: a
/// BlackLevel of whole values and no BlackLevelDeltaH or DeltaV. Other
/// levels (Google's and Sigma's fractional blacks, Canon EOS-1Ds Mark III's
/// per-row deltas) are applied to the samples instead, as they are for
/// OpcodeList2, which works on linear values.
fn whole_levels(plan: &DngPlan) -> bool {
    plan.black.iter().all(|v| v.fract() == 0.)
        && plan
            .black_delta_h
            .iter()
            .chain(&plan.black_delta_v)
            .all(|&d| d == 0.)
}

/// Maps the samples (after the LinearizationTable) to linear reference
/// values as DNG's processing model has it (DNG Specification 1.7.1.0,
/// "Mapping Raw Values to Linear Reference Values"): each pixel's black,
/// BlackLevel's pattern (from the active area's top-left corner) plus its
/// column's BlackLevelDeltaH and its row's BlackLevelDeltaV, is subtracted,
/// and the result scaled by 1 / (WhiteLevel − the largest black), clipped to
/// 0–1 and stored as 0–65535. `area` is the active area (left, top, width,
/// height); the deltas are zero outside it.
fn linearize(
    data: &mut [u16],
    plan: &DngPlan,
    area: [u32; 4],
    white: u32,
    cancel: &AtomicBool,
) -> Result<()> {
    let [rows, cols] = plan.black_dim.unwrap_or([1, 1]).map(|v| v as usize);
    let pattern: Vec<f64> = (0..rows * cols)
        .map(|i| plan.black.get(i).copied().unwrap_or(0.).max(0.))
        .collect();
    let max = |v: &[f64]| {
        v.iter()
            .copied()
            .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
    };
    let largest = max(&pattern).unwrap_or(0.)
        + max(&plan.black_delta_h).unwrap_or(0.)
        + max(&plan.black_delta_v).unwrap_or(0.);
    let range = f64::from(white) - largest;
    if range.is_nan() || range <= 0. {
        return Err(corrupt("DNG white level below its black level"));
    }
    let scale = f64::from(LINEAR_WHITE) / range;
    let [left, top, w, h] = area.map(|v| v as usize);
    let width = plan.width as usize;
    for (y, row) in data.chunks_mut(width).enumerate() {
        if y % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let inside = |i: usize, start: usize, len: usize| i >= start && i < start + len;
        let dv = if inside(y, top, h) {
            plan.black_delta_v.get(y - top).copied().unwrap_or(0.)
        } else {
            0.
        };
        let pattern_row = &pattern[(y + rows - top % rows) % rows * cols..][..cols];
        for (x, v) in row.iter_mut().enumerate() {
            let dh = if inside(x, left, w) {
                plan.black_delta_h.get(x - left).copied().unwrap_or(0.)
            } else {
                0.
            };
            let black = pattern_row[(x + cols - left % cols) % cols] + dh + dv;
            let linear = (f64::from(*v) - black) * scale;
            *v = linear.round().clamp(0., f64::from(LINEAR_WHITE)) as u16;
        }
    }
    Ok(())
}

/// `n` floating-point samples of `bps` bits (IEEE half and single
/// precision; DNG's 24-bit format: sign, 7-bit exponent biased by 63, 16-bit
/// mantissa), in the given byte order, as whole numbers: the levels of
/// floating-point DNGs are on the integer scale.
fn floats(data: &[u8], bps: u32, n: usize, little: bool, out: &mut Vec<u16>) -> Result<()> {
    let size = match bps {
        16 | 24 | 32 => bps as usize / 8,
        _ => return Err(Error::Unsupported(format!("DNG with {bps}-bit floats"))),
    };
    out.clear();
    out.reserve(n);
    for c in data.chunks_exact(size).take(n) {
        let mut b = [0u8; 4];
        for (i, v) in c.iter().enumerate() {
            // Most significant byte first.
            b[i] = if little { c[size - 1 - i] } else { *v };
        }
        let v = match size {
            2 => half(u16::from_be_bytes([b[0], b[1]])),
            3 => fp24([b[0], b[1], b[2]]),
            _ => f32::from_be_bytes(b),
        };
        out.push(to_level(v));
    }
    out.resize(n, 0);
    Ok(())
}

/// A float as a whole-number level: its integer part, as LibRaw's reading of
/// floating-point DNGs has it (our measurement).
fn to_level(v: f32) -> u16 {
    if v.is_nan() {
        0
    } else {
        v.clamp(0., 65535.) as u16
    }
}

/// IEEE 754 half precision.
fn half(h: u16) -> f32 {
    let sign = if h >> 15 == 1 { -1. } else { 1. };
    let exp = i32::from(h >> 10 & 0x1f);
    let mant = f32::from(h & 0x3ff);
    sign * match exp {
        0 => mant * (2f32).powi(-24),
        31 => {
            if mant == 0. {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1. + mant / 1024.) * (2f32).powi(exp - 15),
    }
}

/// DNG's 24-bit float: sign, a 7-bit exponent biased by 63, a 16-bit mantissa.
fn fp24(b: [u8; 3]) -> f32 {
    let sign = if b[0] >> 7 == 1 { -1. } else { 1. };
    let exp = i32::from(b[0] & 0x7f);
    let mant = f32::from(u16::from_be_bytes([b[1], b[2]]));
    sign * match exp {
        0 => mant * (2f32).powi(-62 - 16),
        127 => {
            if mant == 0. {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1. + mant / 65536.) * (2f32).powi(exp - 63),
    }
}

/// Undoes Adobe's floating-point predictor (TIFF Technical Note 3) on rows
/// of `width` samples of `size` bytes: each row's bytes are differences of
/// the byte `factor` positions before, and hold the samples' bytes as planes,
/// most significant first; the result is big-endian samples. DNG names
/// predictors 34894–34896 for factors 1, 2 and 4; the mosaics of the only
/// writer of 34894 in our files (HDRMerge) decode, as LibRaw decodes them,
/// with differences two bytes apart, the same colour's in a Bayer row.
fn unpredict_float(bytes: &mut [u8], width: usize, size: usize, predictor: u32) -> Result<()> {
    let factor = match predictor {
        34894 | 34895 => 2,
        34896 => 4,
        1 => return Ok(()),
        p => return Err(Error::Unsupported(format!("DNG predictor {p}"))),
    };
    let row = width * size;
    let mut plane = vec![0u8; row];
    for r in bytes.chunks_exact_mut(row) {
        for i in factor..row {
            r[i] = r[i].wrapping_add(r[i - factor]);
        }
        plane.copy_from_slice(r);
        for x in 0..width {
            for k in 0..size {
                r[x * size + k] = plane[k * width + x];
            }
        }
    }
    Ok(())
}

/// Uncompressed samples of `bps` bits, packed most significant bit first with
/// rows starting on a byte; 16-bit samples in the file's byte order.
fn unpack(
    data: &[u8],
    bps: u32,
    width: usize,
    rows: usize,
    little: bool,
    out: &mut Vec<u16>,
) -> Result<()> {
    out.clear();
    out.reserve(width * rows);
    match bps {
        8 => out.extend(data.iter().take(width * rows).map(|&b| u16::from(b))),
        16 => out.extend(data.as_chunks::<2>().0.iter().take(width * rows).map(|c| {
            if little {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })),
        1..=15 => {
            let stride = (width * bps as usize).div_ceil(8);
            for row in data.chunks(stride).take(rows) {
                let mut bits = Msb::new(row);
                out.extend((0..width).map(|_| bits.get(bps) as u16));
            }
        }
        _ => return Err(Error::Unsupported(format!("DNG with {bps}-bit samples"))),
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::AtomicBool;

    /// An IFD entry: tag, type, count and value bytes.
    type Entry = (u16, u16, u32, Vec<u8>);

    /// A little-endian DNG of `width × height` 16-bit samples (zeros) in one
    /// strip, its raw IFD0 holding `extra` entries (tag, type, count, value
    /// bytes) too; values over four bytes are stored after the image.
    pub(crate) fn dng(width: u32, height: u32, extra: &[Entry]) -> Vec<u8> {
        let le32 = |v: u32| v.to_le_bytes().to_vec();
        let le16 = |v: u16| v.to_le_bytes().to_vec();
        let mut entries: Vec<Entry> = vec![
            (254, 4, 1, le32(0)),
            (256, 4, 1, le32(width)),
            (257, 4, 1, le32(height)),
            (258, 3, 1, le16(16)),
            (259, 3, 1, le16(1)),
            (262, 3, 1, le16(32803)),
            (277, 3, 1, le16(1)),
            (278, 4, 1, le32(height)),
            (279, 4, 1, le32(width * height * 2)),
            (33421, 3, 2, [le16(2), le16(2)].concat()),
            (33422, 1, 4, vec![0, 1, 1, 2]),
            (50706, 1, 4, vec![1, 4, 0, 0]),
        ];
        entries.extend(extra.iter().cloned());
        entries.sort_by_key(|e| e.0);
        let ifd_at = 8u32;
        let ifd_len = 2 + 12 * (entries.len() as u32 + 1) + 4;
        let image_at = ifd_at + ifd_len;
        let mut values_at = image_at + width * height * 2;
        let mut f = b"II*\0".to_vec();
        f.extend(le32(ifd_at));
        f.extend(le16(entries.len() as u16 + 1));
        let mut values: Vec<u8> = Vec::new();
        let mut put = |f: &mut Vec<u8>, tag: u16, kind: u16, count: u32, v: &[u8]| {
            f.extend(le16(tag));
            f.extend(le16(kind));
            f.extend(le32(count));
            if v.len() <= 4 {
                f.extend(v.iter().copied().chain([0; 4]).take(4));
            } else {
                f.extend(le32(values_at));
                values.extend(v);
                values_at += v.len() as u32;
            }
        };
        for (tag, kind, count, v) in &entries {
            if *tag == 279 {
                put(&mut f, 273, 4, 1, &le32(image_at));
            }
            put(&mut f, *tag, *kind, *count, v);
        }
        f.extend(le32(0));
        f.resize((image_at + width * height * 2) as usize, 0);
        f.extend(values);
        f
    }

    /// `f` (from [`dng`]) with its strip's samples set to `samples`.
    pub(crate) fn with_samples(mut f: Vec<u8>, samples: &[u16]) -> Vec<u8> {
        let count = usize::from(u16::from_le_bytes([f[8], f[9]]));
        let entry = (0..count)
            .map(|i| 10 + 12 * i)
            .find(|&e| u16::from_le_bytes([f[e], f[e + 1]]) == 273)
            .unwrap();
        let at = u32::from_le_bytes(f[entry + 8..entry + 12].try_into().unwrap()) as usize;
        for (i, v) in samples.iter().enumerate() {
            f[at + 2 * i..at + 2 * i + 2].copy_from_slice(&v.to_le_bytes());
        }
        f
    }

    /// Rationals (type 5) or signed rationals (type 10) of `values` / `den`.
    fn rationals(values: &[i32], den: i32) -> Vec<u8> {
        values
            .iter()
            .flat_map(|v| [v.to_le_bytes(), den.to_le_bytes()].concat())
            .collect()
    }

    fn decode(f: &[u8]) -> crate::Sensor {
        let info = crate::open(f).unwrap();
        crate::decode(&info, f, &AtomicBool::new(false)).unwrap()
    }

    /// A fractional BlackLevel (Google's phones, Sigma fp) is not truncated:
    /// the samples become linear reference values, (v − 63.75) / (1023 −
    /// 63.75) on a 16-bit scale, clipped to it.
    #[test]
    fn a_fractional_black_level_maps_samples_to_linear_values() {
        let f = dng(
            5,
            1,
            &[
                (50714, 5, 1, rationals(&[255], 4)),
                (50717, 3, 1, 1023u16.to_le_bytes().to_vec()),
            ],
        );
        let s = decode(&with_samples(f, &[63, 64, 543, 1023, 1100]));
        assert_eq!(s.data, [0, 17, 32742, 65535, 65535]);
        assert_eq!((s.black, s.white), ([0; 4], 65535));
        assert_eq!(s.black_pattern, (0, 0, vec![]));
    }

    /// BlackLevelDeltaH and DeltaV (Canon EOS-1Ds Mark III's per-row black)
    /// add to BlackLevel's pattern, all from the active area's top-left
    /// corner; the scale is 1 / (white − the largest black).
    #[test]
    fn black_level_deltas_are_subtracted_per_row_and_column() {
        let le16 = |v: u16| v.to_le_bytes().to_vec();
        let le32s = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let f = dng(
            4,
            2,
            &[
                // Active area: columns 1–3 of both rows.
                (50829, 4, 4, le32s(&[0, 1, 2, 4])),
                (50713, 3, 2, [le16(1), le16(2)].concat()),
                (50714, 3, 2, [le16(10), le16(20)].concat()),
                (50715, 10, 3, rationals(&[1, 2, 3], 1)),
                (50716, 10, 2, rationals(&[0, 10], 2)),
                (50717, 3, 1, le16(1028)),
            ],
        );
        let s = decode(&with_samples(f, &[528; 8]));
        // Blacks: row 0 20 (masked, the pattern's second column), 11, 22,
        // 13; row 1 each 5 more; the largest is 20 + 3 + 5 = 28, so
        // (528 − black) × 65535 / 1000.
        assert_eq!(
            s.data,
            [33292, 33882, 33161, 33751, 32964, 33554, 32833, 33423]
        );
        assert_eq!((s.black, s.white), ([0; 4], 65535));
        assert_eq!(s.black_pattern, (1, 2, vec![0, 0]));
    }

    /// OpcodeList1 works on the stored image (its coordinates include the
    /// masked column), before the LinearizationTable; OpcodeList2 on the
    /// active area's linear values; OpcodeList2's FixVignetteRadial is left
    /// to development.
    #[test]
    fn opcode_lists_apply_at_their_stages() {
        use super::opcodes::tests::{doubles, gain_map, list, longs};
        let le32s = |v: &[u32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let le16s = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        // MapTable on the stored image's top-left pixel: 0 → 3, 1 → 4.
        let mut table = longs(&[0, 0, 1, 1, 0, 1, 1, 1, 2]);
        table.extend([0, 3, 0, 4]);
        let list1 = list(&[(7, 0, table)]);
        // Gain 2 on the active area's top-left pixel only.
        let list2 = list(&[(9, 1, gain_map([0, 0, 1, 1, 1], [1, 1], &[2.]))]);
        let len = |l: &Vec<u8>| l.len() as u32;
        let f = dng(
            3,
            1,
            &[
                (50829, 4, 4, le32s(&[0, 1, 1, 3])),
                // A LinearizationTable that doubles.
                (50712, 3, 8, le16s(&[0, 2, 4, 6, 8, 10, 12, 14])),
                (50714, 3, 1, le16s(&[0])),
                (50717, 3, 1, le16s(&[14])),
                (51008, 7, len(&list1), list1.clone()),
                (51009, 7, len(&list2), list2),
            ],
        );
        let s = decode(&with_samples(f, &[0, 7, 7]));
        // Stored 0 → 3 (OpcodeList1) → 6 (the table); 7 → 14. As linear
        // values (white 14): 6 / 14, then 1.0 × 2 clipped, and 1.0.
        assert_eq!(s.data, [28086, 65535, 65535]);
        assert_eq!((s.black, s.white), ([0; 4], 65535));
        // FixVignetteRadial in OpcodeList2 alone: the levels stay the file's.
        let mut vignette = doubles(&[1., 0., 0., 0., 0.]);
        vignette.extend(doubles(&[0.5, 0.5]));
        let list2 = list(&[(3, 0, vignette)]);
        let f = dng(2, 1, &[(51009, 7, len(&list2), list2)]);
        let s = decode(&with_samples(f, &[100, 200]));
        assert_eq!((s.data, s.white), (vec![100, 200], 65535));
        assert_eq!(s.black_pattern, (0, 0, vec![]));
        // A list that does not parse is not applied.
        let f = dng(2, 1, &[(51008, 7, 6, list1[..6].to_vec())]);
        assert_eq!(decode(&with_samples(f, &[0, 1])).data, [0, 1]);
    }

    /// The matrix at D65: a D65 calibration's, else interpolated in inverse
    /// temperature between A (2856 K) and D75 (7504 K), else the nearer of
    /// two on one side of D65 (Leica's A and D50: D50's).
    #[test]
    fn colour_matrix_is_taken_at_d65() {
        let ones = Some(vec![1.; 9]);
        let zeros = Some(vec![0.; 9]);
        let at = |ill: [u32; 2]| {
            super::d65_matrix([ones.clone(), zeros.clone()], ill.map(Some)).unwrap()[0][0]
        };
        assert_eq!(at([21, 17]), 1.);
        assert_eq!(at([17, 21]), 0.);
        assert!((at([17, 22]) - 0.094_473_84).abs() < 1e-6);
        assert_eq!(at([17, 23]), 0.);
        assert_eq!(at([20, 17]), 1.);
        // Unknown light: the first; one matrix: that one.
        assert_eq!(at([0, 21]), 1.);
        assert_eq!(
            super::d65_matrix([None, zeros.clone()], [Some(17), Some(21)]).unwrap()[0][0],
            0.
        );
    }

    /// A DNG without Make and Model is named by its UniqueCameraModel.
    #[test]
    fn names_come_from_unique_camera_model_without_make_and_model() {
        let unique = b"Blackmagic Pocket Cinema Camera 4K\0".to_vec();
        let f = dng(2, 1, &[(50708, 2, unique.len() as u32, unique)]);
        let info = crate::open(&f).unwrap();
        assert_eq!(
            [info.make.as_str(), info.model.as_str()],
            ["Blackmagic", "Pocket Cinema Camera 4K"]
        );
        assert_eq!(info.normalized_make, "");
    }

    /// Whole levels stay the file's.
    #[test]
    fn whole_levels_stay_levels() {
        let f = dng(
            2,
            1,
            &[
                (50714, 5, 1, rationals(&[64], 1)),
                (50717, 3, 1, 1023u16.to_le_bytes().to_vec()),
            ],
        );
        let s = decode(&with_samples(f, &[63, 700]));
        assert_eq!(s.data, [63, 700]);
        assert_eq!((s.black, s.white), ([64; 4], 1023));
    }

    #[test]
    fn decodes_floats() {
        assert_eq!(super::half(0x3c00), 1.);
        assert_eq!(super::half(0xc000), -2.);
        assert_eq!(super::half(0x0001), 2f32.powi(-24));
        // 24-bit: 1.5 is exponent 63, mantissa 0x8000.
        assert_eq!(super::fp24([63, 0x80, 0]), 1.5);
        assert_eq!(super::fp24([0x80 | 64, 0, 0]), -2.);
        assert_eq!(super::to_level(2358.53), 2358);
        assert_eq!(super::to_level(-1.), 0);
        assert_eq!(super::to_level(f32::NAN), 0);
        assert_eq!(super::to_level(1e9), 65535);
    }

    #[test]
    fn undoes_the_floating_point_predictor() {
        // Two big-endian 16-bit samples per row, written as byte planes
        // (high bytes, then low) and differenced two bytes apart.
        let samples = [[0x1234u16, 0x5678], [0x9abc, 0xdef0]];
        let mut bytes = Vec::new();
        for row in samples {
            let mut plane: Vec<u8> = row.iter().map(|v| (v >> 8) as u8).collect();
            plane.extend(row.iter().map(|v| *v as u8));
            let mut coded = plane.clone();
            for i in 2..plane.len() {
                coded[i] = plane[i].wrapping_sub(plane[i - 2]);
            }
            bytes.extend(coded);
        }
        super::unpredict_float(&mut bytes, 2, 2, 34894).unwrap();
        let back: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes(*b))
            .collect();
        assert_eq!(back, [0x1234, 0x5678, 0x9abc, 0xdef0]);
    }

    #[test]
    fn deflate_tiles_keep_byte_order_and_precision() {
        use miniz_oxide::deflate::compress_to_vec_zlib;
        let le16 = |v: u16| v.to_le_bytes().to_vec();
        let le32 = |v: u32| v.to_le_bytes().to_vec();
        // A 4 × 1 image in one 4 × 1 tile; `data` is the tile before Deflate.
        let file = |bps: u16, format: u16, predictor: u16, data: &[u8]| {
            let tile = compress_to_vec_zlib(data, 6);
            let mut f = dng(
                4,
                1,
                &[
                    (258, 3, 1, le16(bps)),
                    (259, 3, 1, le16(8)),
                    (317, 3, 1, le16(predictor)),
                    (339, 3, 1, le16(format)),
                    (322, 4, 1, le32(4)),
                    (323, 4, 1, le32(1)),
                    (324, 4, 1, le32(0)),
                    (325, 4, 1, le32(tile.len() as u32)),
                ],
            );
            // The tile goes last; its offset entry (inline) is set to it.
            let at = f.len() as u32;
            let count = usize::from(u16::from_le_bytes([f[8], f[9]]));
            let entry = (0..count)
                .map(|i| 10 + 12 * i)
                .find(|&e| u16::from_le_bytes([f[e], f[e + 1]]) == 324)
                .unwrap();
            f[entry + 8..entry + 12].copy_from_slice(&at.to_le_bytes());
            f.extend(&tile);
            f
        };
        let decode = |f: Vec<u8>| {
            let info = crate::open(&f).unwrap();
            crate::decode(&info, &f, &AtomicBool::new(false))
                .unwrap()
                .data
        };
        // Little-endian floats with no predictor keep the file's byte order.
        let floats: Vec<u8> = [1f32, 2., 100., 2358.53]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(decode(file(32, 3, 1, &floats)), [1, 2, 100, 2358]);
        // 8-bit horizontal differences wrap at 8 bits: 250, then +10 is 4.
        assert_eq!(decode(file(8, 1, 2, &[250, 10, 0, 1])), [250, 4, 4, 5]);
    }

    #[test]
    fn decodes_a_small_dng() {
        let f = dng(4, 2, &[]);
        let info = crate::open(&f).unwrap();
        assert_eq!((info.width, info.height), (4, 2));
        let s = crate::decode(&info, &f, &AtomicBool::new(false)).unwrap();
        assert_eq!(s.data, vec![0; 8]);
    }

    #[test]
    fn hostile_sizes_are_errors_not_panics() {
        let le32 = |v: u32| v.to_le_bytes().to_vec();
        let rational = |n: u32| [n.to_le_bytes(), 1u32.to_le_bytes()].concat();
        let cases: Vec<Vec<Entry>> = vec![
            // Tiles of the largest size.
            vec![
                (322, 4, 1, le32(u32::MAX)),
                (323, 4, 1, le32(u32::MAX)),
                (324, 4, 1, le32(8)),
                (325, 4, 1, le32(8)),
            ],
            // A black pattern of 65536 × 65536.
            vec![(50713, 4, 2, [le32(65536), le32(65536)].concat())],
            // A default crop beyond the sensor.
            vec![
                (50719, 5, 2, [rational(u32::MAX), rational(0)].concat()),
                (50720, 5, 2, [rational(4), rational(2)].concat()),
            ],
        ];
        for extra in cases {
            let f = dng(4, 2, &extra);
            if let Ok(info) = crate::open(&f) {
                let _ = crate::decode(&info, &f, &AtomicBool::new(false));
            }
        }
    }
}
