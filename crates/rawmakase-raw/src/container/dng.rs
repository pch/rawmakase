//! Adobe DNG (DNG Specification 1.7): the raw image directory, its tiles or
//! strips and their compression, the CFA layout, levels and linearization, the
//! active area and default crop, and the colour tags.
use super::tiff::{Ifd, TiffFile};
use crate::codec::{bits::Msb, ljpeg};
use crate::error::{Error, Result, corrupt};
use crate::info::{Cfa, RawInfo, UNSET};
use crate::io::{Bytes, Order};
use crate::sensor::Sensor;
use crate::variant::{Plan, Variant};
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a DNG's raw image is and how it is coded.
#[derive(Clone, Debug, Default)]
pub(crate) struct DngPlan {
    /// The levels: BlackLevelRepeatDim (rows, columns), BlackLevel, and
    /// WhiteLevel.
    pub black_dim: Option<[u32; 2]>,
    pub black: Vec<f64>,
    pub white: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub bps: u32,
    pub compression: u32,
    /// Tile (or strip) size; strips are tiles as wide as the image.
    pub tile: [u32; 2],
    /// Each tile's (offset, length).
    pub tiles: Tiles,
    pub little: bool,
    /// LinearizationTable: stored value to linear value.
    pub curve: Vec<u16>,
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
    // Apple's DNGs name the camera module (UniqueCameraModel), not the phone.
    if info.make.trim() == "Apple"
        && let Some(unique) = t.text(ifd0, 50708).filter(|u| !u.trim().is_empty())
    {
        info.model = unique;
    }
    info.flip = crate::geometry::flip(t.int(ifd0, 274).unwrap_or(1));
    if let Some(neutral) = t.reals(ifd0, 50728).filter(|n| n.len() == 3) {
        for (mul, n) in info.cam_mul.iter_mut().zip(&neutral) {
            if *n > 0. {
                *mul = (1. / n) as f32;
            }
        }
        info.cam_mul[3] = info.cam_mul[1];
    }
    // The colour matrix for D65 (illuminant 21), else the only one.
    let illuminants = [t.int(ifd0, 50778), t.int(ifd0, 50779)];
    let which = if illuminants[1] == Some(21) { 1 } else { 0 };
    let matrix = t
        .reals(ifd0, [50721, 50722][which])
        .or_else(|| t.reals(ifd0, 50721))
        .filter(|m| m.len() == 9)
        .map(|m| std::array::from_fn(|r| std::array::from_fn(|c| m[r * 3 + c] as f32)));
    info.file_matrix = matrix;
    super::tiff_exif::read(&t, &mut info);
    info.previews = t.previews();
    info.uncompressed_preview = t.has_uncompressed_thumbnail();
    let black_dim = t
        .ints(ifd, 50713)
        .filter(|d| d.len() == 2 && (1..=16).contains(&d[0]) && (1..=16).contains(&d[1]))
        .map(|d| [d[0], d[1]]);
    let black = t.reals(ifd, 50714).unwrap_or_default();
    let white = t.int(ifd, 50717);
    info.plan = Plan::Dng(DngPlan {
        black_dim,
        black,
        white,
        width,
        height,
        bps,
        compression,
        tile,
        tiles,
        little: t.little(),
        curve,
    });
    let _ = bytes;
    Ok(info)
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
        let samples: &[u16] = match plan.compression {
            1 => {
                unpack(tile, plan.bps, tw, th, plan.little, &mut decoded)?;
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
    if !plan.curve.is_empty() {
        let last = plan.curve.len() - 1;
        for v in &mut data {
            *v = plan.curve[usize::from(*v).min(last)];
        }
    }
    let [left, top] = info.margins;
    let (black, pattern) = crate::levels::dng_black(plan.black_dim, &plan.black);
    let white = plan
        .white
        .unwrap_or_else(|| ((1u64 << plan.bps.min(16)) - 1) as u32);
    Ok(Sensor {
        width: plan.width,
        height: plan.height,
        channels: 1,
        data,
        area: [left, top, info.width, info.height],
        black,
        black_pattern: pattern,
        white,
        linear_white: [0; 4],
    })
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
mod tests {
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
