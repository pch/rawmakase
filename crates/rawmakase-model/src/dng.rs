//! Rendering hints that a DNG carries for its raw image, as Lightroom uses them:
//! the embedded camera profile, BaselineExposure, the default crop, and the
//! FixVignetteRadial (OpcodeList2), and GainMap and WarpRectilinear (OpcodeList3) lens
//! corrections.
use crate::{
    optics::{GainGrid, LensCorrection, Radial},
    tiff::{Entry, Tiff},
};
use std::{collections::BTreeMap, fs::File, path::Path};

#[derive(Default)]
pub struct Dng {
    pub baseline_exposure: Option<f32>,
    pub neutral_calibration: Option<crate::camera_data::NeutralCalibration>,
    pub profile_calibration_signature: Option<String>,
    /// Left, top, width, height, relative to the active area.
    pub crop: Option<[u32; 4]>,
    /// DefaultUserCrop as `[left, top, right, bottom]` fractions of the default crop:
    /// the crop Camera Raw starts from, such as the camera's aspect ratio.
    pub user_crop: Option<[f32; 4]>,
    /// The profile tags, rewritten as a standalone DCP: the embedded camera
    /// profile when they make one, else at least the file's colour matrices.
    /// `None` when IFD0 has no colour tags (colour or forward matrices, tables).
    pub profile: Option<Vec<u8>>,
    /// Profile tags are there but one could not be read (an offset past the end):
    /// `profile` is then `None`, which must not read as no profile.
    pub profile_unreadable: bool,
    pub lens: Option<LensCorrection>,
}

/// IFD0 tags that make up an embedded camera profile, in DCP form.
const PROFILE_TAGS: &[u16] = &[
    50708, // UniqueCameraModel
    50721, 50722, // ColorMatrix1/2
    50778, 50779, // CalibrationIlluminant1/2
    50932, // ProfileCalibrationSignature
    50936, // ProfileName
    50937, 50938, 50939, // ProfileHueSatMap dims, data 1/2
    50940, // ProfileToneCurve
    50941, // ProfileEmbedPolicy
    50942, // ProfileCopyright
    50964, 50965, // ForwardMatrix1/2
    50981, 50982, // ProfileLookTable dims, data
    51107, 51108, // ProfileHueSatMapEncoding, ProfileLookTableEncoding
    51109, // BaselineExposureOffset
    51110, // DefaultBlackRender
];

/// `None` unless the file is a DNG.
pub fn read(path: &Path) -> Option<Dng> {
    let mut t = Tiff::open(File::open(path).ok()?, 0)?;
    let ifd0 = t.ifd(t.first)?;
    ifd0.get(&50706)?; // DNGVersion
    let profile = profile_tags(&mut t, &ifd0);
    let mut dng = Dng {
        baseline_exposure: ifd0
            .get(&50730)
            .and_then(|e| t.numbers(e))
            .and_then(|v| v.first().copied())
            .filter(|v| v.is_finite() && v.abs() <= 5.),
        profile_unreadable: profile.is_err(),
        profile: profile.unwrap_or_default(),
        profile_calibration_signature: ifd0
            .get(&50932)
            .and_then(|entry| String::from_utf8(t.raw(entry)?).ok())
            .map(|value| value.trim_end_matches('\0').to_owned()),
        ..Default::default()
    };
    // The full-resolution raw image is the SubIFD (or IFD0) with NewSubfileType 0.
    let mut candidates = vec![ifd0];
    if let Some(subs) = candidates[0].get(&330).and_then(|e| t.offsets(e)) {
        for offset in subs.into_iter().take(16) {
            if let Some(ifd) = t.ifd(offset) {
                candidates.push(ifd);
            }
        }
    }
    // Strips or tiles, and NewSubfileType 0 (the main image) when recorded.
    let at = candidates.iter().position(|ifd| {
        (ifd.contains_key(&273) || ifd.contains_key(&324))
            && ifd
                .get(&254)
                .is_none_or(|e| t.numbers(e).is_some_and(|v| v.first() == Some(&0.)))
    })?;
    // Linear raw's AnalogBalance is the decoder's, applied apart
    // (`Metadata::dng_analog_balance`); a mosaic's keeps its calibration out.
    let linear = candidates[at]
        .get(&262)
        .and_then(|e| t.numbers(e))
        .is_some_and(|v| v.first() == Some(&34892.));
    dng.neutral_calibration = neutral_calibration(&mut t, &candidates[0], linear);
    let raw = candidates.swap_remove(at);
    let pair = |t: &mut Tiff, tag| -> Option<[u32; 2]> {
        let v = t.numbers(raw.get(&tag)?)?;
        (v.len() == 2 && v.iter().all(|x| x.is_finite() && *x >= 0.))
            .then(|| [v[0].round() as u32, v[1].round() as u32])
    };
    if let (Some(origin), Some(size)) = (pair(&mut t, 50719), pair(&mut t, 50720)) {
        dng.crop = Some([origin[0], origin[1], size[0], size[1]]);
    }
    // Top, left, bottom, right.
    if let Some(&[top, left, bottom, right]) = raw
        .get(&51125)
        .and_then(|e| t.numbers(e))
        .as_deref()
        .filter(|v| v.iter().all(|x| (0. ..=1.).contains(x)))
        && right - left >= 0.01
        && bottom - top >= 0.01
    {
        dng.user_crop = Some([left, top, right, bottom]);
    }
    // OpcodeList2 and 3 apply to the active area (or the whole stored image), which
    // their optical centres are fractions of, whose size OpcodeList3's GainMaps must
    // cover and which the decoder decodes.
    let area = raw.get(&50829).and_then(|e| t.numbers(e));
    let size = match area.as_deref() {
        Some(&[top, left, bottom, right]) if bottom > top && right > left => {
            Some([(bottom - top) as u32, (right - left) as u32])
        }
        _ => {
            let dim = |t: &mut Tiff, tag| Some(*t.numbers(raw.get(&tag)?)?.first()? as u32);
            dim(&mut t, 257).zip(dim(&mut t, 256)).map(|(h, w)| [h, w])
        }
    };
    let frame = size.map(|[h, w]| [w as f32, h as f32]);
    let vignetting = raw
        .get(&51009)
        .and_then(|e| t.raw(e))
        .and_then(|b| opcode(&b, 3))
        .and_then(|p| vignette(&p, frame));
    let list3 = raw.get(&51022).and_then(|e| t.raw(e));
    let warp = list3
        .as_deref()
        .and_then(|b| opcode(b, 1))
        .and_then(|p| warp(&p, frame));
    let gain_map = list3
        .as_deref()
        .zip(size)
        .and_then(|(b, size)| gain_grid(b, size));
    // One centre for the correction: the warp's, which moves the image. A vignette
    // centred elsewhere (no file seen) is left out rather than moved.
    let centre = match (&warp, &vignetting) {
        (Some((_, _, c)), _) | (None, Some((_, c))) => *c,
        (None, None) => LensCorrection::image_centre(),
    };
    let vignetting = vignetting
        .filter(|(_, c)| frame.is_none_or(|[w, h]| same_centre(*c, centre, [w, h])))
        .map(|(v, _)| v);
    let (distortion, chromatic) = warp.map_or((None, None), |(d, c, _)| (d, c));
    let lens = LensCorrection {
        source: "DNG opcodes".into(),
        default_on: true,
        vignetting,
        distortion,
        chromatic,
        chromatic_at_source: false,
        gain_map,
        centre,
    };
    dng.lens = (!lens.is_empty() && lens.validate()).then_some(lens);
    Some(dng)
}

fn neutral_calibration(
    t: &mut Tiff,
    ifd: &BTreeMap<u16, Entry>,
    linear: bool,
) -> Option<crate::camera_data::NeutralCalibration> {
    // Absence means no per-file correction. The DNG discriminator still prevents
    // falling back to a native camera's calibration.
    if !ifd.contains_key(&50723) && !ifd.contains_key(&50724) {
        return None;
    }
    let identity = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
    let mut numbers = |tag, default: &[f32]| match ifd.get(&tag) {
        Some(entry) => t.numbers(entry),
        None => Some(default.to_vec()),
    };
    let a = numbers(50723, &identity)?;
    let b = numbers(50724, &identity)?;
    let analog = if linear {
        vec![1.; 3]
    } else {
        numbers(50727, &[1.; 3])?
    };
    let gains = constant_diagonal(&a, &b, &analog)?;
    let signature = match ifd.get(&50931) {
        Some(entry) => String::from_utf8(t.raw(entry)?)
            .ok()?
            .trim_end_matches('\0')
            .to_owned(),
        None => String::new(),
    };
    Some(crate::camera_data::NeutralCalibration { gains, signature })
}

/// Do not flatten illuminant-dependent or non-diagonal matrices into three gains.
fn constant_diagonal(a: &[f32], b: &[f32], analog: &[f32]) -> Option<[f32; 3]> {
    if a.len() != 9 || a != b || analog != [1.; 3] || [1, 2, 3, 5, 6, 7].iter().any(|i| a[*i] != 0.)
    {
        return None;
    }
    let gains = [a[0], a[4], a[8]];
    gains
        .iter()
        .all(|v| v.is_finite() && *v > 0. && *v < 4.)
        .then_some(gains)
}

/// Tags that give a profile colour: colour and forward matrices and tables.
const COLOUR_TAGS: &[u16] = &[50721, 50722, 50938, 50939, 50964, 50965, 50982];

/// Rewrites the profile tags as a standalone DCP so the regular parser validates them.
/// Returns the bytes rather than a profile, since the matrix is readable from
/// them even when the profile itself is not; `None` without colour tags, an error
/// when a tag that is there can't be read.
fn profile_tags(t: &mut Tiff, ifd0: &BTreeMap<u16, Entry>) -> Result<Option<Vec<u8>>, ()> {
    if !COLOUR_TAGS.iter().any(|tag| ifd0.contains_key(tag)) {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for tag in PROFILE_TAGS {
        if let Some(e) = ifd0.get(tag) {
            entries.push((*tag, e.kind, e.count, t.raw(e).ok_or(())?));
        }
    }
    let le = t.little;
    let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut out: Vec<u8> = if le { b"II".to_vec() } else { b"MM".to_vec() };
    out.extend(u16b(0x4352)); // DCP magic
    out.extend(u32b(8));
    out.extend(u16b(entries.len() as u16));
    let mut data_at = 8 + 2 + entries.len() * 12 + 4;
    let mut data: Vec<u8> = Vec::new();
    for (tag, kind, count, bytes) in &entries {
        out.extend(u16b(*tag));
        out.extend(u16b(*kind));
        out.extend(u32b(*count));
        if bytes.len() <= 4 {
            let mut v = bytes.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            out.extend(u32b(data_at as u32));
            data.extend(bytes);
            if bytes.len() % 2 == 1 {
                data.push(0);
            }
            data_at = 8 + 2 + entries.len() * 12 + 4 + data.len();
        }
    }
    out.extend(u32b(0));
    out.extend(data);
    Ok(Some(out))
}

/// Parameters of the first opcode with `id` in a big-endian DNG opcode list.
fn opcode(b: &[u8], id: u32) -> Option<Vec<u8>> {
    opcodes(b, id).into_iter().next()
}
/// The ids of a big-endian DNG opcode list's opcodes, in order, up to the first that
/// does not parse.
fn opcode_ids(b: &[u8]) -> Vec<u32> {
    let be = |o: usize| Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?));
    let mut ids = Vec::new();
    let mut o = 4;
    for _ in 0..be(0).unwrap_or(0).min(64) {
        let Some((code, size)) = be(o).zip(be(o + 12)) else {
            break;
        };
        let end = o.saturating_add(16).saturating_add(size as usize);
        if b.len() < end {
            break;
        }
        ids.push(code);
        o = end;
    }
    ids
}
/// Parameters of every opcode with `id` in a big-endian DNG opcode list, in order, up
/// to the first that does not parse.
fn opcodes(b: &[u8], id: u32) -> Vec<Vec<u8>> {
    let be = |o: usize| Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?));
    let mut found = Vec::new();
    let mut o = 4;
    for _ in 0..be(0).unwrap_or(0).min(64) {
        let Some((code, size)) = be(o).zip(be(o + 12)) else {
            break;
        };
        let end = o.saturating_add(16).saturating_add(size as usize);
        let Some(params) = b.get(o + 16..end) else {
            break;
        };
        if code == id {
            found.push(params.to_vec());
        }
        o = end;
    }
    found
}
/// OpcodeList3's GainMaps (opcode 9) as one gain per channel, on an image of `size`
/// [height, width]. Each must cover a rectangle inside the image, every row and column,
/// with planes among the three channels, each plane at most once; maps for separate
/// planes must share one grid and area, and a warp (opcode 1) may come before them or
/// after them, not between.
/// Anything else is not read, and the photo renders as before.
fn gain_grid(list: &[u8], [height, width]: [u32; 2]) -> Option<GainGrid> {
    let after_warp = opcode_ids(list)
        .iter()
        .position(|id| *id == 1)
        .zip(opcode_ids(list).iter().position(|id| *id == 9))
        .is_some_and(|(warp, gain)| warp < gain);
    if opcode_ids(list)
        .iter()
        .skip_while(|id| **id != 9)
        .skip_while(|id| **id == 9)
        .any(|id| *id == 9)
    {
        return None;
    }
    let mut grid: Option<GainGrid> = None;
    let mut per_plane: [Option<Vec<f32>>; 3] = Default::default();
    for p in opcodes(list, 9) {
        let long = |i: usize| {
            Some(u32::from_be_bytes(
                p.get(i * 4..i * 4 + 4)?.try_into().ok()?,
            ))
        };
        let double = |o: usize| Some(f64::from_be_bytes(p.get(o..o + 8)?.try_into().ok()?));
        let header: Vec<u32> = (0..10).map(long).collect::<Option<_>>()?;
        let &[
            top,
            left,
            bottom,
            right,
            plane,
            planes,
            row_pitch,
            col_pitch,
            rows,
            columns,
        ] = header.as_slice()
        else {
            return None;
        };
        let [spacing_v, spacing_h, origin_v, origin_h] =
            [40, 48, 56, 64].map(|o| double(o).map(|v| v as f32));
        let map_planes = long(18)?;
        let (rows, columns) = (rows as usize, columns as usize);
        let gains: Vec<f32> = p
            .get(76..)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_be_bytes(*c))
            .collect();
        if top >= bottom
            || left >= right
            || bottom > height
            || right > width
            || (row_pitch, col_pitch) != (1, 1)
            || planes == 0
            || plane >= 3
            || planes > 3 - plane
            || map_planes == 0
            || map_planes > planes
            || !(1..=256).contains(&rows)
            || !(1..=256).contains(&columns)
            || gains.len() != rows * columns * map_planes as usize
        {
            return None;
        }
        let shape = GainGrid {
            area: [
                top as f32 / height as f32,
                left as f32 / width as f32,
                bottom as f32 / height as f32,
                right as f32 / width as f32,
            ],
            after_warp,
            points: [rows, columns],
            origin: [origin_v?, origin_h?],
            spacing: [spacing_v?, spacing_h?],
            planes: 1,
            gains: Vec::new(),
        };
        if grid.as_ref().is_some_and(|g| *g != shape) {
            return None;
        }
        grid = Some(shape);
        for c in plane..plane + planes {
            // The DNG SDK reads the map's plane of the image plane itself, the last
            // one for planes beyond the map's.
            let m = c.min(map_planes - 1) as usize;
            // Two maps on one plane multiply sample by sample, which a single grid
            // holds only where their points coincide: not read.
            if per_plane[c as usize].is_some() {
                return None;
            }
            per_plane[c as usize] = Some(gains.chunks(map_planes as usize).map(|g| g[m]).collect());
        }
    }
    let mut grid = grid?;
    let points = grid.points[0] * grid.points[1];
    let one = vec![1.; points];
    let planes: [&Vec<f32>; 3] = std::array::from_fn(|c| per_plane[c].as_ref().unwrap_or(&one));
    grid.planes = 3;
    grid.gains = (0..points).flat_map(|i| planes.map(|p| p[i])).collect();
    // A map the lens correction does not hold is left out on its own: the file's
    // warp and vignetting still apply.
    (grid.valid() && grid.gains.iter().any(|g| (g - 1.).abs() > 1e-4)).then_some(grid)
}
fn doubles(p: &[u8]) -> Vec<f64> {
    p.as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_be_bytes(*c))
        .collect()
}
/// An opcode's optical centre (fractions of the image of `frame` pixels) and the
/// factor from our radius, 1 at the half diagonal, to the opcode's, 1 at the image's
/// corner farthest from that centre (the DNG SDK's normalization), with our knots out
/// to that corner. Without the image's size only a centred opcode is read.
fn opcode_frame(cx: f64, cy: f64, frame: Option<[f32; 2]>) -> Option<([f32; 2], f32, Vec<f32>)> {
    let centre = [cx as f32, cy as f32];
    let Some(size) = frame else {
        return ((cx - 0.5).abs() < 0.02 && (cy - 0.5).abs() < 0.02)
            .then(|| (LensCorrection::image_centre(), 1., knots(1.)));
    };
    if !centre.iter().all(|c| (0. ..=1.).contains(c)) {
        return None;
    }
    let reach = crate::optics::reach(centre, size);
    Some((centre, 1. / reach, knots(reach)))
}
fn knots(reach: f32) -> Vec<f32> {
    (0..=32).map(|i| i as f32 / 32. * reach).collect()
}
/// Two optical centres within half a pixel of each other on an image of `size`.
fn same_centre(a: [f32; 2], b: [f32; 2], size: [f32; 2]) -> bool {
    ((a[0] - b[0]) * size[0]).hypot((a[1] - b[1]) * size[1]) <= 0.5
}
/// FixVignetteRadial: gain 1 + k0 r² + k1 r⁴ + k2 r⁶ + k3 r⁸ + k4 r¹⁰, and its centre.
fn vignette(p: &[u8], frame: Option<[f32; 2]>) -> Option<(Radial, [f32; 2])> {
    let v = doubles(p);
    if v.len() != 7 {
        return None;
    }
    let (centre, to_opcode, knots) = opcode_frame(v[5], v[6], frame)?;
    let values = knots
        .iter()
        .map(|r| {
            let r2 = f64::from(*r * to_opcode).powi(2);
            (1. + (0..5).map(|i| v[i] * r2.powi(i as i32 + 1)).sum::<f64>()) as f32
        })
        .collect();
    let r = Radial { knots, values };
    r.values
        .iter()
        .any(|g| (g - 1.).abs() > 1e-4)
        .then_some((r, centre))
}
/// The distortion and lateral CA of a WarpRectilinear warp, whose source radius ratio
/// is kr0 + kr1 r² + kr2 r⁴ + kr3 r⁶ per plane, and its centre. Tangential terms are
/// ignored.
type Warp = (Option<Radial>, Option<[Radial; 2]>, [f32; 2]);
fn warp(p: &[u8], frame: Option<[f32; 2]>) -> Option<Warp> {
    let planes = u32::from_be_bytes(p.get(..4)?.try_into().ok()?) as usize;
    let v = doubles(p.get(4..)?);
    if !matches!(planes, 1 | 3) || v.len() != planes * 6 + 2 {
        return None;
    }
    let (centre, to_opcode, knots) = opcode_frame(v[planes * 6], v[planes * 6 + 1], frame)?;
    let ratio = |plane: usize, r: f32| {
        let k = &v[plane * 6..plane * 6 + 4];
        let r2 = f64::from(r * to_opcode).powi(2);
        (k[0] + k[1] * r2 + k[2] * r2 * r2 + k[3] * r2 * r2 * r2) as f32
    };
    let green = if planes == 3 { 1 } else { 0 };
    let curve = |f: &dyn Fn(f32) -> f32| Radial {
        knots: knots.clone(),
        values: knots.iter().map(|r| f(*r)).collect(),
    };
    let distortion = curve(&|r| ratio(green, r));
    let chromatic = (planes == 3).then(|| {
        [
            curve(&|r| ratio(0, r) / ratio(1, r)),
            curve(&|r| ratio(2, r) / ratio(1, r)),
        ]
    });
    let changes = |r: &Radial| r.values.iter().any(|v| (v - 1.).abs() > 1e-6);
    Some((
        Some(distortion).filter(changes),
        chromatic.filter(|[a, b]| changes(a) || changes(b)),
        centre,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_per_file_calibration_and_does_not_flatten_complex_matrices() {
        let matrix: [f32; 9] = [0.9817, 0., 0., 0., 1., 0., 0., 0., 0.9687];
        let rationals = |v: &[f32]| {
            v.iter()
                .flat_map(|v| {
                    let mut b = ((*v * 10000.).round() as i32).to_le_bytes().to_vec();
                    b.extend(10000i32.to_le_bytes());
                    b
                })
                .collect::<Vec<_>>()
        };
        let file = |analog: [f32; 3], photometric: u16| {
            let entries = [
                (262u16, 3u16, 1u32, photometric.to_le_bytes().to_vec()),
                (273, 4, 1, 0u32.to_le_bytes().to_vec()),
                (50706, 1, 4, vec![1, 4, 0, 0]),
                (50723, 10, 9, rationals(&matrix)),
                (50724, 10, 9, rationals(&matrix)),
                (50727, 10, 3, rationals(&analog)),
                (50931, 2, 10, b"com.adobe\0".to_vec()),
            ];
            let mut bytes = b"II*\0\x08\0\0\0".to_vec();
            bytes.extend((entries.len() as u16).to_le_bytes());
            let mut data: Vec<u8> = Vec::new();
            for (tag, kind, count, value) in &entries {
                bytes.extend(tag.to_le_bytes());
                bytes.extend(kind.to_le_bytes());
                bytes.extend(count.to_le_bytes());
                if value.len() <= 4 {
                    bytes.extend(value.iter().copied().chain([0; 4]).take(4));
                } else {
                    let at = 8 + 2 + entries.len() * 12 + 4 + data.len();
                    bytes.extend((at as u32).to_le_bytes());
                    data.extend(value);
                }
            }
            bytes.extend(0u32.to_le_bytes());
            bytes.extend(data);
            let f = tempfile::NamedTempFile::new().unwrap();
            std::fs::write(f.path(), bytes).unwrap();
            read(f.path()).unwrap().neutral_calibration
        };
        let calibration = file([1.; 3], 32803).unwrap();
        assert_eq!(calibration.gains, [0.9817, 1., 0.9687]);
        assert_eq!(calibration.signature, "com.adobe");
        // A mosaic's AnalogBalance keeps its calibration out; linear raw's is
        // applied apart, so its calibration stays.
        assert!(file([1., 2., 1.], 32803).is_none());
        let linear = file([1., 2., 1.], 34892).unwrap();
        assert_eq!(linear.gains, [0.9817, 1., 0.9687]);
        let mut other = matrix;
        other[0] = 0.99;
        assert!(constant_diagonal(&matrix, &other, &[1.; 3]).is_none());
        other = matrix;
        other[1] = 0.1;
        assert!(constant_diagonal(&other, &other, &[1.; 3]).is_none());
        assert!(constant_diagonal(&matrix, &matrix, &[1., 2., 1.]).is_none());
        other = matrix;
        other[0] = f32::NAN;
        assert!(constant_diagonal(&other, &other, &[1.; 3]).is_none());
    }
    fn list(id: u32, params: &[u8]) -> Vec<u8> {
        let mut b = 1u32.to_be_bytes().to_vec();
        for v in [id, 0x0103_0000, 0, params.len() as u32] {
            b.extend(v.to_be_bytes());
        }
        b.extend(params);
        b
    }
    fn be(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
    #[test]
    fn decodes_vignette_and_warp_opcodes() {
        // DSCF7853's FixVignetteRadial from Lightroom's DNG.
        let p = be(&[0.2599, 0.3424, 0.0630, 0.0346, -0.0300, 0.5, 0.5]);
        let (v, _) = vignette(&opcode(&list(3, &p), 3).unwrap(), None).unwrap();
        assert!((v.eval(1.) - 1.67).abs() < 0.01);
        assert!((v.eval(0.5) - 1.087).abs() < 0.002);
        assert!(opcode(&list(3, &p), 1).is_none());
        let mut w = 3u32.to_be_bytes().to_vec();
        w.extend(be(&[1.0002, 0.0005, 0., 0., 0., 0.]));
        w.extend(be(&[1., 0., 0., 0., 0., 0.]));
        w.extend(be(&[1., 0., 0., 0., 0., 0.]));
        w.extend(be(&[0.5, 0.5]));
        let (distortion, chromatic, _) = warp(&w, None).unwrap();
        assert!(distortion.is_none());
        let [red, blue] = chromatic.unwrap();
        assert!((red.eval(1.) - 1.0007).abs() < 1e-5);
        assert_eq!(blue.eval(1.), 1.);
        // Off-centre corrections need the image's size, to normalize the radius.
        assert!(vignette(&be(&[0.2, 0., 0., 0., 0., 0.3, 0.5]), None).is_none());
    }
    /// An optical centre off the image's: the radius is 1 at the image's corner farthest
    /// from it, as the DNG SDK normalizes it (Adobe's DNG of a Panasonic LX100, whose
    /// warp is centred 8 px left of its 4144 px active area's centre; a Z 6's 7.5 px
    /// left and 26 px down).
    #[test]
    fn reads_off_centre_opcodes_about_their_centre() {
        let size = [4144f32, 3104.];
        let (cx, cy) = (0.4980694980694981, 0.5);
        let mut w = 1u32.to_be_bytes().to_vec();
        w.extend(be(&[0.99995, -0.19355, 0.07209, -0.01145, 0., 0.]));
        w.extend(be(&[cx, cy]));
        let (distortion, _, centre) = warp(&w, Some(size)).unwrap();
        assert_eq!(centre, [cx as f32, cy as f32]);
        let distortion = distortion.unwrap();
        let half = 4144f64.hypot(3104.) / 2.;
        let far = (cx * 4144.).max((1. - cx) * 4144.).hypot(1552.);
        let opcode = |q: f64| 0.99995 - 0.19355 * q * q + 0.07209 * q.powi(4) - 0.01145 * q.powi(6);
        for px in [500., 1500., 2500., far] {
            let ours = f64::from(distortion.eval((px / half) as f32));
            assert!((ours - opcode(px / far)).abs() < 2e-4, "{px}: {ours}");
        }
        // Knots reach the far corner.
        assert!(*distortion.knots.last().unwrap() as f64 >= far / half - 1e-4);
        let p = be(&[0.3, 0.1, 0., 0., 0., 0.46, 0.53]);
        let (v, centre) = vignette(&p, Some([6000., 4000.])).unwrap();
        assert_eq!(centre, [0.46, 0.53]);
        let far = (0.54f64 * 6000.).hypot(0.53 * 4000.);
        let half = 6000f64.hypot(4000.) / 2.;
        let at = f64::from(v.eval((far / half) as f32));
        assert!((at - 1.4).abs() < 1e-3, "{at}");
        // A centre outside the image is not one.
        assert!(vignette(&be(&[0.2, 0., 0., 0., 0., 1.3, 0.5]), Some(size)).is_none());
    }
    /// A GainMap opcode's parameters: `area` (top, left, bottom, right), its planes,
    /// a `rows` × `columns` map over the whole area and `map_planes` gains per point.
    fn gain_map(
        area: [u32; 4],
        plane: u32,
        planes: u32,
        map_planes: u32,
        gains: &[f32],
    ) -> Vec<u8> {
        let (rows, columns) = (2u32, 3u32);
        let mut p: Vec<u8> = area
            .into_iter()
            .chain([plane, planes, 1, 1, rows, columns])
            .flat_map(u32::to_be_bytes)
            .collect();
        p.extend(be(&[1., 0.5, 0., 0.]));
        p.extend(map_planes.to_be_bytes());
        p.extend(gains.iter().flat_map(|g| g.to_be_bytes()));
        p
    }
    fn lists(ops: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut b = (ops.len() as u32).to_be_bytes().to_vec();
        for (id, params) in ops {
            for v in [*id, 0x0103_0000, 0, params.len() as u32] {
                b.extend(v.to_be_bytes());
            }
            b.extend(params);
        }
        b
    }
    #[test]
    fn reads_opcode_list3_gain_maps_per_channel() {
        let area = [0, 0, 300, 400];
        // One map for all three planes, as DJI and Hasselblad drones write it, before
        // the warp.
        let rgb: Vec<f32> = (0..6)
            .flat_map(|i| [1. + i as f32 * 0.1, 1., 1.5])
            .collect();
        let warp = {
            let mut w = 1u32.to_be_bytes().to_vec();
            w.extend(be(&[1., 0.01, 0., 0., 0., 0., 0.5, 0.5]));
            w
        };
        let list = lists(&[(9, gain_map(area, 0, 3, 3, &rgb)), (1, warp.clone())]);
        let grid = gain_grid(&list, [300, 400]).unwrap();
        assert_eq!((grid.points, grid.planes), ([2, 3], 3));
        assert_eq!(grid.gain(0., 0.), [1., 1., 1.5]);
        // Towards the last point, and halfway along the first row: bilinear.
        let [r, ..] = grid.gain(0.9999, 0.9999);
        assert!((r - 1.5).abs() < 1e-3);
        let [r, ..] = grid.gain(0., 0.25);
        assert!((r - 1.05).abs() < 1e-6);
        // Outside the image, nothing.
        assert_eq!(grid.gain(1., 0.5), [1.; 3]);
        // A map per plane, and a plane without one keeps its level.
        let list = lists(&[
            (9, gain_map(area, 0, 1, 1, &[2.; 6])),
            (9, gain_map(area, 2, 1, 1, &[3.; 6])),
        ]);
        assert_eq!(
            gain_grid(&list, [300, 400]).unwrap().gain(0.5, 0.5),
            [2., 1., 3.]
        );
        // A map of one plane for all three.
        let list = lists(&[(9, gain_map(area, 0, 3, 1, &[2.; 6]))]);
        assert_eq!(
            gain_grid(&list, [300, 400]).unwrap().gain(0.5, 0.5),
            [2.; 3]
        );
        // Planes from 1: each reads the map's plane of its own index, as the DNG SDK
        // does, the map's last for planes beyond it.
        let two: Vec<f32> = (0..6).flat_map(|_| [2., 3.]).collect();
        let list = lists(&[(9, gain_map(area, 1, 2, 2, &two))]);
        assert_eq!(
            gain_grid(&list, [300, 400]).unwrap().gain(0.5, 0.5),
            [1., 3., 3.]
        );
        // A map of part of the image, after the warp (the Sigma fp's): no gain outside
        // its area, and its points over the area.
        let list = lists(&[
            (1, warp.clone()),
            (9, gain_map([30, 40, 270, 360], 0, 3, 3, &rgb)),
        ]);
        let grid = gain_grid(&list, [300, 400]).unwrap();
        assert!(grid.after_warp);
        assert_eq!(grid.gain(0.05, 0.5), [1.; 3]);
        // Inside it, the points are over the whole image, as without an area.
        assert_eq!(grid.gain(0.5, 0.5), [1.25, 1., 1.5]);
        assert!(
            !gain_grid(&lists(&[(9, gain_map(area, 0, 3, 3, &rgb))]), [300, 400])
                .unwrap()
                .after_warp
        );
        // Grids beyond 256 points a side, however large, are not read.
        let mut huge = gain_map(area, 0, 3, 3, &[2.; 18]);
        huge[32..40].copy_from_slice(&[u32::MAX.to_be_bytes(), u32::MAX.to_be_bytes()].concat());
        assert!(gain_grid(&lists(&[(9, huge)]), [300, 400]).is_none());
        // Planes beyond the image's three, however large, are not read.
        assert!(
            gain_grid(
                &lists(&[(9, gain_map(area, u32::MAX, 2, 1, &[2.; 6]))]),
                [300, 400]
            )
            .is_none()
        );
        // Maps beyond the image, with a warp between them, or of gains 1, are not read.
        for (list, size) in [
            (lists(&[(9, gain_map(area, 0, 3, 3, &rgb))]), [290, 400]),
            (
                lists(&[
                    (9, gain_map(area, 0, 1, 1, &[2.; 6])),
                    (1, warp),
                    (9, gain_map(area, 1, 1, 1, &[2.; 6])),
                ]),
                [300, 400],
            ),
            (lists(&[(9, gain_map(area, 0, 3, 1, &[1.; 6]))]), [300, 400]),
            // Two maps on one plane, and gains a lens correction does not hold.
            (
                lists(&[
                    (9, gain_map(area, 0, 1, 1, &[2.; 6])),
                    (9, gain_map(area, 0, 1, 1, &[3.; 6])),
                ]),
                [300, 400],
            ),
            (lists(&[(9, gain_map(area, 0, 3, 1, &[9.; 6]))]), [300, 400]),
        ] {
            assert!(gain_grid(&list, size).is_none());
        }
    }
    #[test]
    fn non_dng_files_are_ignored() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"II*\0\x08\0\0\0\0\0\0\0").unwrap();
        assert!(read(f.path()).is_none());
    }
}
