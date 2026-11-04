//! Maker lens-correction tables, read directly from the RAW container.
//!
//! - Fujifilm RAF: FujiIFD tags 0xF00B (distortion), 0xF00F (lateral CA) and
//!   0xF010 (vignetting), signed rationals. The first value is a pixel scale; knots
//!   are fractions of the half diagonal, distortion is in percent, CA a radius
//!   fraction, vignetting the remaining illumination in percent.
//! - Sony ARW: raw SubIFD tags 0x7032 (vignetting), 0x7035 (CA) and 0x7037
//!   (distortion), signed shorts led by the number of values and padded with zeros
//!   to a fixed length (ZV-1: 11 distortion knots in 16 slots). Knots are spread
//!   evenly from the centre to the corner. Distortion is in 2^-14, CA in 2^-21
//!   units; vignetting v restores 2^(2^(v/8192 - 1) - 0.5). Lightroom applies the CA,
//!   and a fixed-lens compact's distortion, from them; all three stand in for an
//!   Adobe profile ([`Builtin`]).
//! - Olympus and OM System ORF: the maker note's ImageProcessing directory (0x2040),
//!   tags 0x150A (distortion: k1, k2, k3 and a scale s, floats) and 0x150C (lateral
//!   CA: three coefficients for red, then three for blue, floats). With radius 1 at
//!   the half diagonal of the camera's default crop, an output radius r samples
//!   green at s·r·(1 + k1 (s·r)² + k2 (s·r)⁴ + k3 (s·r)⁶), and red and blue at that
//!   radius times 1 + c0 + c1 ρ² + c2 ρ⁴, ρ being green's source radius.
//! - Panasonic RW2: IFD0 tag 0x0119, sixteen signed shorts (ExifTool's
//!   DistortionInfo). Value 7's low four bits are 1 when the correction applies,
//!   value 12 is the radius unit N in pixels, and values 8, 4 and 11 over 2^15 are
//!   a, b and c of the camera's own correction, source radius s to output radius
//!   s·(1 + a s² + b s⁴ + c s⁶). Value 5 scales it: the output radius r samples the
//!   source radius that this maps to r·(1 + v5/2^15).
//! - Panasonic RW2 lateral CA: IFD0 tag 0x011B, 32 signed shorts. Value 14 is 256 when
//!   the table holds data (0 for lenses without). Values 17, 16, 4 and 11 are four
//!   source radii in pixels, the last about the half diagonal; values 12, 23, 8 and 26
//!   (red) and 27, 10, 20 and 29 (blue) are the channel's displacement from green at
//!   those radii in −1/1000 px, so red and blue sample green's source radius plus it.
//!   The other values, many of which change from shot to shot, are not used.
//! - Nikon NEF: tag 0xC7D5 of the raw SubIFD (ExifTool's NikonNEFInfo), a TIFF after a
//!   "Nikon\0" header and four version bytes, whose entries 5 to 8 are records led by
//!   "0100": distortion, vignetting, lateral CA and the optical centre. Values are
//!   signed rationals, polynomials of the radius r with coefficients for descending
//!   powers down to r¹ (`descending`), r being 1 at the default crop's corner farthest
//!   from the optical centre. Distortion, after a mode byte and a count of 4 at 0x10:
//!   green's source radius over r is (1 + k(r)) / (1 + x), x at 0x40. Vignetting, a
//!   count of 8 at 0x10: gain 1 + a·v(r), a at 0x60. CA, for red at 0x10 and blue at
//!   0x34: a count of 3, the coefficients and a constant e, the channel's scale over
//!   green's at r being 1 + e + c(r). The centre: x and y in pixels from the image's.
//!   A Coolpix NRW's NEFInfo holds tables in records 2 and 3 instead
//!   (`coolpix_tables`).
//! - Samsung SRW: the maker note's Distortion (0xA050, eight signed longs) and
//!   ChromaticAberration (0xA051, 22 shorts), each value offset by one of the eleven
//!   of its EncryptionKey (0xA020). Distortion: four mantissas, each followed by a
//!   power of ten, of green's source radius over the output radius r in pixels,
//!   c0 + c1 r² + c2 r⁴ + c3 r⁶. CA: a count of 7, the radii in pixels, then red's and
//!   blue's scale relative to green at each, in 1/10⁴.
//! - Canon CR3 (EOS R5 Mark II and R6 Mark III): record 14 of the timed metadata (the
//!   `CTMD` track's sample), little-endian shorts: three counts of knots, then seven
//!   arrays of 23 slots, vignetting (the light kept, in 2^-13) and its knots, red and
//!   blue (their scale relative to green, 2^14 + 2^23·(s − 1)) and their knots,
//!   distortion (green's source radius over the output radius, in 2^-14) and its
//!   knots. Knots are radii in units of 5.314 µm on the sensor, from 0 at the centre.
//!   The distortion is applied as Adobe fits it (`canon_distortion`).
//!
//! The Fujifilm vignetting agrees with the FixVignetteRadial opcode Adobe writes into
//! DNGs of the same file to within 1%, and the Olympus and Panasonic distortion and
//! CA with the WarpRectilinear opcodes (and, for newer bodies, the embedded lens
//! profiles) Adobe DNG Converter 18.7 writes, to within a pixel, Nikon's with its
//! opcodes and profiles to within 0.4 px, Samsung's with its opcodes to within
//! 0.15 px and Canon's with its profiles and warps to within 0.2 px (see
//! docs/lens-corrections.md).
use crate::camera_data::Metadata;
use crate::optics::{LensCorrection, Radial};
use crate::tiff::{Entry, Tiff, kind};
use std::{
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
};

/// The lens correction a RAW stores.
#[derive(Debug, Default)]
pub(crate) struct Builtin {
    /// What Lightroom applies from the file whatever Enable Profile Corrections says
    /// (or, for a correction that is not `default_on`, when it is on).
    pub lens: Option<LensCorrection>,
    /// The camera's fuller data, standing in for an Adobe lens profile when Enable
    /// Profile Corrections is on and none is imported: Sony's tables, of which
    /// Lightroom itself applies only the lateral CA (and a compact's distortion).
    pub stand_in: Option<LensCorrection>,
}

/// The built-in correction that applies without Enable Profile Corrections; `None`
/// for unsupported files or when the camera stored none. `m` is the decoded image the
/// radii are fractions of: its size and default crop.
#[cfg(test)]
fn read(path: &Path, m: &Metadata) -> Option<LensCorrection> {
    read_builtin(path, m).lens
}

/// What `path` stores of the camera's lens correction ([`Builtin`]).
pub(crate) fn read_builtin(path: &Path, m: &Metadata) -> Builtin {
    let valid = |c: Option<LensCorrection>| c.filter(|c| !c.is_empty() && c.validate());
    let b = read_any(path, m).unwrap_or_default();
    Builtin {
        lens: valid(b.lens),
        stand_in: valid(b.stand_in),
    }
}

fn read_any(path: &Path, m: &Metadata) -> Option<Builtin> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 108];
    f.read_exact(&mut head).ok()?;
    let c = if head.starts_with(b"FUJIFILMCCD-RAW") {
        let base = u32::from_be_bytes(head[100..104].try_into().ok()?) as u64;
        fuji(&mut Tiff::open(f, base)?)
    } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        let mut t = Tiff::open(f, 0)?;
        let make = make(&mut t);
        if make.starts_with("nikon") {
            nikon(&mut t, m)
        } else if make.starts_with("samsung") {
            samsung(path, &mut t, m)
        } else {
            return sony(&mut t);
        }
    } else if [b"IIRO", b"IIRS", b"MMOR"]
        .iter()
        .any(|h| head.starts_with(*h))
    {
        olympus(path, &mut Tiff::open(f, 0)?, m)
    } else if head.starts_with(b"IIU\0") {
        panasonic(&mut Tiff::open(f, 0)?, m)
    } else if head.get(4..12) == Some(b"ftypcrx ".as_slice()) {
        canon(&mut f, m)
    } else {
        None
    }?;
    Some(Builtin {
        lens: Some(c),
        stand_in: None,
    })
}

const FUJI_VIGNETTE_STRENGTH: f32 = 0.85;

fn fuji(t: &mut Tiff) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let fuji = t.ifd(t.offset(ifd0.get(&0xf000)?)?)?;
    let mut numbers = |tag| fuji.get(&tag).and_then(|e| t.numbers(e));
    let distortion = numbers(0xf00b);
    let chromatic = numbers(0xf00f);
    let vignetting = numbers(0xf010);
    // [scale, n knots, n values]
    let pairs = |v: &[f32], f: &dyn Fn(f32) -> f32| {
        let n = v.len().checked_sub(1)? / 2;
        (n >= 2 && v.len() == 2 * n + 1).then(|| {
            Radial::new(
                v[1..=n].to_vec(),
                v[n + 1..].iter().map(|x| f(*x)).collect(),
            )
        })?
    };
    let vignetting = vignetting
        // Lightroom renders Fujifilm's table about 15% weaker in log gain: corners of
        // three X100F photos match Camera Raw at exponent 0.85 (+0.08 EV at 1).
        .and_then(|v| pairs(&v, &|p| (100. / p).powf(FUJI_VIGNETTE_STRENGTH)))
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-4));
    // Scaled, as Adobe's warps of it are, so that its widest ratio is 1: the table
    // carries no fill scale of its own (X-Pro2: Adobe's k₀ 0.99042, 1 / 1.0098 here).
    let distortion = distortion
        .and_then(|v| pairs(&v, &|p| 1. + p / 100.))
        .map(|mut r| {
            let widest = r.values.iter().copied().fold(1f32, f32::max);
            r.values.iter_mut().for_each(|v| *v /= widest);
            r
        })
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-6));
    // [scale, n knots, n red, n blue]
    let chromatic = chromatic.and_then(|v| {
        let n = v.len().checked_sub(1)? / 3;
        if n < 2 || v.len() != 3 * n + 1 {
            return None;
        }
        let knots = v[1..=n].to_vec();
        let red = Radial::new(
            knots.clone(),
            v[n + 1..=2 * n].iter().map(|x| 1. + x).collect(),
        )?;
        let blue = Radial::new(knots, v[2 * n + 1..].iter().map(|x| 1. + x).collect())?;
        [&red, &blue]
            .iter()
            .any(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-7))
            .then_some([red, blue])
    });
    Some(LensCorrection {
        source: "Fujifilm built-in".into(),
        default_on: true,
        vignetting,
        distortion,
        chromatic,
        ..Default::default()
    })
}

/// Sony models whose stored distortion Lightroom applies: the fixed-lens compacts
/// (RX100, RX10, RX0, HX, ZV-1), not the RX1 series, which Adobe corrects with its own
/// profile only, nor bodies with interchangeable lenses.
fn sony_applies_distortion(model: &str) -> bool {
    let m = (model.trim_matches(|c: char| c == '\0' || c.is_whitespace())).to_ascii_uppercase();
    (m.starts_with("DSC-") && m != "DSC-RX1" && !m.starts_with("DSC-RX1R")) || m.starts_with("ZV-1")
}

fn sony(t: &mut Tiff) -> Option<Builtin> {
    let ifd0 = t.ifd(t.first)?;
    let model = (ifd0.get(&0x0110).and_then(|e| t.raw(e)))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let sub = t.ifd(t.offset(ifd0.get(&0x14a)?)?)?;
    let mut numbers = |tag| sub.get(&tag).and_then(|e| t.numbers(e));
    let vignetting = numbers(0x7032);
    let chromatic = numbers(0x7035);
    let distortion = numbers(0x7037);
    let knots = |n: usize| {
        (0..n)
            .map(|i| i as f32 / (n - 1) as f32)
            .collect::<Vec<_>>()
    };
    // The count's values, from storage that may be longer: a ZV-1 stores 11 distortion
    // knots in 16 slots. Anything stored past the count must be zero padding.
    let counted = |v: Option<Vec<f32>>, per_count: usize| {
        let v = v?;
        let count = *v.first()? as usize;
        let values = v.get(1..=count)?;
        (count >= 2 * per_count
            && count.is_multiple_of(per_count)
            && v[count + 1..].iter().all(|x| *x == 0.))
        .then(|| values.to_vec())
    };
    let single = |v: Option<Vec<f32>>, f: &dyn Fn(f32) -> f32| {
        let v = counted(v, 1)?;
        Radial::new(knots(v.len()), v.iter().map(|x| f(*x)).collect())
    };
    let vignetting = single(vignetting, &|v| (2f32.powf(v / 8192. - 1.) - 0.5).exp2())
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-4));
    // A natural cubic spline through the knots, as the camera's correction is smooth:
    // against Adobe's warp it halves the difference on the RX100 III to V and ZV-1.
    let distortion = counted(distortion, 1)
        .and_then(|v| {
            let v: Vec<f64> = v.iter().map(|d| 1. + f64::from(*d) / 16384.).collect();
            let at = spline(&v);
            Radial::new(
                self::knots(1.),
                self::knots(1.)
                    .into_iter()
                    .map(|r| at(f64::from(r)) as f32)
                    .collect(),
            )
        })
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-6));
    let chromatic = counted(chromatic, 2).and_then(|v| {
        let n = v.len() / 2;
        let scale = |s: &[f32]| s.iter().map(|x| 1. + x / 2_097_152.).collect();
        Some([
            Radial::new(knots(n), scale(&v[..n]))?,
            Radial::new(knots(n), scale(&v[n..]))?,
        ])
    });
    let stand_in = LensCorrection {
        source: LensCorrection::SONY_BUILTIN.into(),
        default_on: true,
        vignetting,
        distortion,
        chromatic,
        ..Default::default()
    };
    // Adobe DNG Converter writes the lateral CA of every Sony body with tables, and a
    // compact's distortion, as opcodes, which Lightroom always applies; never the
    // vignetting.
    let lens = LensCorrection {
        vignetting: None,
        distortion: (stand_in.distortion.clone()).filter(|_| sony_applies_distortion(&model)),
        ..stand_in.clone()
    };
    Some(Builtin {
        lens: Some(lens),
        stand_in: Some(stand_in),
    })
}

/// A natural cubic spline through `v`, at knots spread evenly over [0, 1].
fn spline(v: &[f64]) -> impl Fn(f64) -> f64 + '_ {
    let n = v.len();
    let h = 1. / (n - 1) as f64;
    // Second derivatives, zero at both ends (Thomas algorithm).
    let mut m = vec![0.; n];
    if n > 2 {
        let (mut c, mut d) = (vec![0.; n], vec![0.; n]);
        for i in 1..n - 1 {
            let rhs = 6. * (v[i + 1] - 2. * v[i] + v[i - 1]) / (h * h);
            let w = 4. - c[i - 1];
            c[i] = 1. / w;
            d[i] = (rhs - d[i - 1]) / w;
        }
        for i in (1..n - 1).rev() {
            m[i] = d[i] - c[i] * m[i + 1];
        }
    }
    move |r: f64| {
        let i = ((r / h) as usize).min(n - 2);
        let t = r - i as f64 * h;
        let u = h - t;
        (m[i] * u * u * u + m[i + 1] * t * t * t) / (6. * h)
            + (v[i] / h - m[i] * h / 6.) * u
            + (v[i + 1] / h - m[i + 1] * h / 6.) * t
    }
}

/// Radius samples from the centre out to `reach`, the decoded image's corner farthest
/// from it (`optics::reach`).
fn knots(reach: f32) -> Vec<f32> {
    (0..=32).map(|i| i as f32 / 32. * reach).collect()
}

/// The centre of the camera's default crop, in fractions of the decoded image (its
/// centre without a crop).
fn crop_centre(m: &Metadata) -> [f32; 2] {
    if m.crop_width == 0 || m.crop_height == 0 || m.width == 0 || m.height == 0 {
        return LensCorrection::image_centre();
    }
    [
        (m.crop_left as f32 + m.crop_width as f32 * 0.5) / m.width as f32,
        (m.crop_top as f32 + m.crop_height as f32 * 0.5) / m.height as f32,
    ]
}

/// A maker's radial model in its own radius unit, `unit` pixels, from the optical
/// `centre` (fractions of the decoded image), sampled at the decoded image's radii:
/// `green` maps an output radius to green's source radius, `chromatic` red's and
/// blue's scale relative to green at green's source radius.
fn maker_model(
    m: &Metadata,
    unit: f64,
    centre: [f32; 2],
    green: &dyn Fn(f64) -> Option<f64>,
    chromatic: Option<[&dyn Fn(f64) -> f64; 2]>,
) -> (Option<Radial>, Option<[Radial; 2]>) {
    let half = f64::from(m.width).hypot(f64::from(m.height)) * 0.5;
    if !(half > 0. && unit > 0.) {
        return (None, None);
    }
    let knots = || {
        knots(crate::optics::reach(
            centre,
            [m.width as f32, m.height as f32],
        ))
    };
    // The ratio's limit at the centre is its value just beside it.
    let x = |r: f32| (f64::from(r) * half / unit).max(1e-4);
    let sources: Option<Vec<f64>> = knots().into_iter().map(|r| green(x(r))).collect();
    let Some(sources) = sources.filter(|s| s.windows(2).all(|p| p[0] < p[1])) else {
        return (None, None);
    };
    let curve = |f: &dyn Fn(usize, f64) -> f64| {
        let values = (knots().into_iter().enumerate())
            .map(|(i, r)| f(i, x(r)) as f32)
            .collect();
        Radial::new(knots(), values)
    };
    let changes = |r: &Radial, by: f32| r.values.iter().any(|v| (v - 1.).abs() > by);
    let distortion = curve(&|i, x| sources[i] / x).filter(|r| changes(r, 1e-6));
    let chromatic = chromatic
        .and_then(|[red, blue]| {
            Some([
                curve(&|i, _| red(sources[i]))?,
                curve(&|i, _| blue(sources[i]))?,
            ])
        })
        .filter(|[a, b]| changes(a, 1e-7) || changes(b, 1e-7));
    (distortion, chromatic)
}

/// The values of a FLOAT entry.
fn floats(t: &mut Tiff, e: &Entry) -> Option<Vec<f64>> {
    if e.kind != kind::FLOAT {
        return None;
    }
    let b = t.raw(e)?;
    let v: Vec<f64> = (b.as_chunks::<4>().0.iter())
        .map(|c| f64::from(f32::from_bits(t.u32(c))))
        .collect();
    v.iter().all(|x| x.is_finite()).then_some(v)
}

fn olympus(path: &Path, t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let exif = t.ifd(t.offset(ifd0.get(&0x8769)?)?)?;
    let note = exif.get(&0x927c)?;
    let at = t.u32(&note.value) as u64;
    // Offsets in the note count from its start, after its own byte order mark.
    let head = t.bytes(at, 16)?;
    let (start, source) = if head.starts_with(b"OLYMPUS\0") {
        (8, "Olympus built-in")
    } else if head.starts_with(b"OM SYSTEM\0\0\0") {
        (12, "OM System built-in")
    } else {
        return None;
    };
    let little = match &head[start..start + 2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut n = Tiff::headless(File::open(path).ok()?, at, little);
    let main = n.ifd(start as u64 + 4)?;
    let e = main.get(&0x2040)?;
    let processing = match e.kind {
        // Older notes store the directory inline.
        kind::UNDEFINED => n.u32(&e.value) as u64,
        _ => n.offset(e)?,
    };
    let ip = n.ifd(processing)?;
    let d = floats(&mut n, ip.get(&0x150a)?).filter(|d| d.len() == 4)?;
    let ca: Option<[f64; 6]> = ip
        .get(&0x150c)
        .and_then(|e| floats(&mut n, e))
        .and_then(|c| c.try_into().ok());
    let s = if d[3] > 0. { d[3] } else { 1. };
    let k = [d[0], d[1], d[2]];
    let green = |r: f64| {
        let x = s * r;
        let x2 = x * x;
        Some(x * (1. + x2 * (k[0] + x2 * (k[1] + x2 * k[2]))))
    };
    // Red's and blue's scale at green's source radius ρ.
    let scale = |at: usize, rho: f64| {
        ca.map_or(1., |c| {
            1. + c[at] + rho * rho * (c[at + 1] + rho * rho * c[at + 2])
        })
    };
    let (red, blue) = (|rho| scale(0, rho), |rho| scale(3, rho));
    let chromatic = ca.is_some().then_some([&red as &dyn Fn(f64) -> f64, &blue]);
    let (w, h) = match (m.crop_width, m.crop_height) {
        (0, _) | (_, 0) => (m.width, m.height),
        size => size,
    };
    let unit = f64::from(w).hypot(f64::from(h)) * 0.5;
    // Centred on the default crop, as Adobe's warps are: the decoded image has more
    // unused columns on one side (the E-M10 Mark III's centre is 8 px off).
    let centre = crop_centre(m);
    let (distortion, chromatic) = maker_model(m, unit, centre, &green, chromatic);
    Some(LensCorrection {
        source: source.into(),
        // Adobe applies it as the photo's lens profile: written into its DNGs as
        // their warp, or for newer bodies as the "Camera Settings" profile.
        default_on: true,
        vignetting: None,
        distortion,
        chromatic,
        chromatic_at_source: false,
        gain_map: None,
        centre,
    })
}

/// The values of an IFD0 entry of `n` signed shorts.
fn shorts<const N: usize>(t: &mut Tiff, e: &Entry) -> Option<[f64; N]> {
    let b = t.raw(e)?;
    let v: Vec<f64> = (b.get(..2 * N)?.as_chunks::<2>().0.iter())
        .map(|c| f64::from(t.u16(c) as i16))
        .collect();
    v.try_into().ok()
}

/// Panasonic's lateral CA (IFD0 0x011B) as red's and blue's scale relative to green,
/// each a function of green's source radius in pixels. `half` is the decoded image's
/// half diagonal.
fn panasonic_ca(v: [f64; 32], half: f64) -> Option<[impl Fn(f64) -> f64; 2]> {
    let knots = [v[17], v[16], v[4], v[11]];
    if v[14] != 256. || knots[0] <= 0. || !knots.is_sorted_by(|a, b| a < b) || half <= 0. {
        return None;
    }
    let at = |i: [usize; 4]| i.map(|i| -v[i] / 1000.);
    let (red, blue) = (at([12, 23, 8, 26]), at([27, 10, 20, 29]));
    if red.iter().chain(&blue).all(|d| *d == 0.) {
        return None;
    }
    // Displacement in pixels at a source radius: straight lines between the knots,
    // from none at the centre, held past the last knot.
    let lines = |d: [f64; 4], g: f64| {
        let i = knots.iter().position(|k| g < *k).unwrap_or(4);
        let (k0, d0) = if i == 0 {
            (0., 0.)
        } else {
            (knots[i - 1], d[i - 1])
        };
        match knots.get(i) {
            Some(k1) => d0 + (d[i] - d0) * (g - k0) / (k1 - k0),
            None => d0,
        }
    };
    // Adobe applies a smooth fit of those lines in its warp's form, 1 + c0 + c1 s² +
    // c2 s⁴ + c3 s⁶ with s = g / half. Least squares with residuals weighted by s, out
    // to the half diagonal, matches the CA in its DNGs of 309 photos from 111 bodies
    // to within 0.15 px (median 0.015 px); the lines themselves differ by up to 0.86 px.
    let fit = |d: [f64; 4]| {
        let mut normal = [[0.; 5]; 4];
        for i in 1..=256 {
            let s = f64::from(i) / 256.;
            let s2 = s * s;
            let row = [s, s * s2, s * s2 * s2, s * s2 * s2 * s2];
            let y = lines(d, s * half) / half;
            for (j, n) in normal.iter_mut().enumerate() {
                for k in 0..4 {
                    n[k] += row[j] * row[k];
                }
                n[4] += row[j] * y;
            }
        }
        let c = solve4(normal)?;
        Some(move |g: f64| {
            let s2 = (g / half).powi(2);
            1. + c[0] + s2 * (c[1] + s2 * (c[2] + s2 * c[3]))
        })
    };
    Some([fit(red)?, fit(blue)?])
}

/// Solves the 4×4 system in the augmented matrix `m` by Gaussian elimination with
/// partial pivoting, or `None` when it is singular.
fn solve4(mut m: [[f64; 5]; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|a, b| m[*a][col].abs().total_cmp(&m[*b][col].abs()))?;
        m.swap(col, pivot);
        if m[col][col].abs() < 1e-300 {
            return None;
        }
        let top = m[col];
        for row in &mut m[col + 1..] {
            let f = row[col] / top[col];
            for (x, t) in row.iter_mut().zip(top).skip(col) {
                *x -= f * t;
            }
        }
    }
    let mut x = [0.; 4];
    for row in (0..4).rev() {
        let rest: f64 = (row + 1..4).map(|k| m[row][k] * x[k]).sum();
        x[row] = (m[row][4] - rest) / m[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

fn panasonic(t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let info = ifd0.get(&0x0119).and_then(|e| shorts::<16>(t, e));
    let ca = ifd0.get(&0x011b).and_then(|e| shorts::<32>(t, e));
    let half = f64::from(m.width).hypot(f64::from(m.height)) * 0.5;
    let ca = ca.and_then(|v| panasonic_ca(v, half));
    // Value 7's low four bits switch the distortion correction on.
    let info = info.filter(|v| (v[7] as i16 as u16) & 0xf == 1 && v[12] > 0.);
    // Radii in the distortion model's unit, or in pixels without it.
    let unit = info.map_or(1., |v| v[12]);
    let [a, b, c] = info.map_or([0.; 3], |v| [v[8], v[4], v[11]].map(|x| x / 32768.));
    let scale = info.map_or(1., |v| 1. + v[5] / 32768.);
    // The camera's correction maps the source radius to the output radius; its
    // inverse, by Newton's method, is green's source radius for an output radius.
    let forward = |s: f64| {
        let s2 = s * s;
        s * (1. + s2 * (a + s2 * (b + s2 * c)))
    };
    let slope = |s: f64| {
        let s2 = s * s;
        1. + s2 * (3. * a + s2 * (5. * b + s2 * 7. * c))
    };
    let green = |r: f64| {
        let target = r * scale;
        let mut s = target;
        for _ in 0..50 {
            let d = slope(s);
            if d <= 0. {
                return None;
            }
            s -= (forward(s) - target) / d;
        }
        ((forward(s) - target).abs() < 1e-9 && s > 0.).then_some(s)
    };
    // The CA applies at green's source radius, in the sensor's pixels.
    let [red, blue] = match &ca {
        Some([red, blue]) => [Some(red), Some(blue)],
        None => [None, None],
    };
    let red = |rho: f64| red.map_or(1., |f| f(rho * unit));
    let blue = |rho: f64| blue.map_or(1., |f| f(rho * unit));
    let chromatic = ca.is_some().then_some([&red as &dyn Fn(f64) -> f64, &blue]);
    if info.is_none() && chromatic.is_none() {
        return None;
    }
    // Adobe's warps are centred on the default crop.
    let centre = crop_centre(m);
    let (distortion, chromatic) = maker_model(m, unit, centre, &green, chromatic);
    let source = if make(t).starts_with("leica") {
        "Leica built-in"
    } else {
        "Panasonic built-in"
    };
    Some(LensCorrection {
        source: source.into(),
        default_on: true,
        vignetting: None,
        distortion,
        chromatic,
        chromatic_at_source: false,
        gain_map: None,
        centre,
    })
}

/// IFD0's Make, in lower case.
fn make(t: &mut Tiff) -> String {
    t.ifd(t.first)
        .and_then(|ifd0| t.raw(ifd0.get(&0x010f)?))
        .map(|b| String::from_utf8_lossy(&b).to_ascii_lowercase())
        .unwrap_or_default()
}

/// The value of a polynomial whose coefficients are for descending powers of `q`,
/// the last for q¹, as Nikon stores them.
fn descending(c: &[f64], q: f64) -> f64 {
    c.iter().fold(0., |sum, k| (sum + k) * q)
}

fn nikon(t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    // In the raw image's SubIFD.
    let info = (t.offsets(ifd0.get(&0x14a)?)?.into_iter()).find_map(|at| {
        let sub = t.ifd(at)?;
        t.raw(sub.get(&0xc7d5)?)
    })?;
    if !info.starts_with(b"Nikon\0") {
        return None;
    }
    let mut n = Tiff::open(Cursor::new(info), 10)?;
    let dir = n.ifd(n.first)?;
    let mut record = |tag, len| {
        (dir.get(&tag).and_then(|e| n.raw(e))).filter(|b| b.len() >= len && b.starts_with(b"0100"))
    };
    let (d, v, ca, centre) = (
        record(5, 0x48),
        record(6, 0x68),
        record(7, 0x58),
        record(8, 0x14),
    );
    // Coolpix NRW files hold tables in records 2 and 3 instead.
    let coolpix = (record(2, 8 + 8 * 17), record(3, 8 + 8 * 34));
    let little = n.little;
    let int = |b: &[u8], at: usize| {
        let c = b.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(c)
        } else {
            u32::from_be_bytes(c)
        })
    };
    let rational = |b: &[u8], at: usize| {
        let (num, den) = (int(b, at)? as i32, int(b, at + 4)? as i32);
        (den != 0).then(|| f64::from(num) / f64::from(den))
    };
    // A count of `n` rationals at `at`, followed by them.
    let coefficients = |b: &[u8], at: usize, n: u32| -> Option<Vec<f64>> {
        if int(b, at)? != n {
            return None;
        }
        (0..n as usize)
            .map(|i| rational(b, at + 4 + 8 * i))
            .collect()
    };
    let Some(d) = d else {
        let (Some(distortion), Some(ca)) = coolpix else {
            return None;
        };
        return coolpix_tables(&distortion, &ca, m, &rational);
    };
    // Bit 0 of the mode is the camera's Auto Distortion Control, bit 1 a lens that
    // needs the correction. Adobe applies nothing, not even vignetting, without the
    // first (D6, D780).
    let mode = d[4];
    if mode & 1 == 0 {
        return None;
    }
    let k = coefficients(&d, 0x10, 4)?;
    let scale = 1. / (1. + rational(&d, 0x40)?);
    // The optical centre, which tag 8 places in pixels from the image centre, as
    // Adobe's opcodes do (Z 6 DSC_0750: −7.52, +26.30). The radius is 1 at the default
    // crop's corner farthest from it.
    let [dx, dy] = [4, 12].map(|at| centre.as_ref().and_then(|c| rational(c, at)).unwrap_or(0.));
    let (iw, ih) = (f64::from(m.width), f64::from(m.height));
    let (cx, cy) = (iw * 0.5 + dx, ih * 0.5 + dy);
    let (left, top, w, h) = match (m.crop_width, m.crop_height) {
        (0, _) | (_, 0) => (0., 0., iw, ih),
        _ => (
            f64::from(m.crop_left),
            f64::from(m.crop_top),
            f64::from(m.crop_width),
            f64::from(m.crop_height),
        ),
    };
    let unit = (cx - left)
        .max(left + w - cx)
        .hypot((cy - top).max(top + h - cy));
    let half = iw.hypot(ih) * 0.5;
    if !(half > 0. && unit > 0. && scale.is_finite() && scale > 0.) {
        return None;
    }
    let centre = [(cx / iw) as f32, (cy / ih) as f32];
    let knots = || knots(crate::optics::reach(centre, [iw as f32, ih as f32]));
    let curve = |f: &dyn Fn(f64) -> f64| {
        let values = knots()
            .into_iter()
            .map(|r| f(f64::from(r) * half / unit) as f32);
        Radial::new(knots(), values.collect())
    };
    let changes = |r: &Radial, by: f32| r.values.iter().any(|v| (v - 1.).abs() > by);
    let distortion = curve(&|q| scale * (1. + descending(&k, q))).filter(|r| changes(r, 1e-6));
    let vignetting = v
        .and_then(|v| {
            let c = coefficients(&v, 0x10, 8)?;
            let strength = rational(&v, 0x60)?;
            curve(&|q| 1. + strength * descending(&c, q))
        })
        .filter(|r| changes(r, 1e-4));
    // Red's and blue's scale at the output radius: a constant after the coefficients.
    let chromatic = ca
        .and_then(|b| {
            let plane = |at: usize| {
                let c = coefficients(&b, at, 3)?;
                let constant = rational(&b, at + 28)?;
                curve(&|q| 1. + constant + descending(&c, q))
            };
            Some([plane(0x10)?, plane(0x34)?])
        })
        .filter(|[a, b]| changes(a, 1e-7) || changes(b, 1e-7));
    Some(LensCorrection {
        source: "Nikon built-in".into(),
        // Adobe writes a lens that needs it as its DNGs' opcodes, which always apply.
        default_on: mode & 2 != 0,
        vignetting,
        distortion,
        chromatic,
        chromatic_at_source: false,
        gain_map: None,
        centre,
    })
}

/// Red's and blue's displacement from green in a Coolpix CA table, as a fraction of
/// the half diagonal: measured against Adobe's warps of the P950, P1000 and P1100,
/// where it is the same to within 1% (`docs/lens-corrections.md`).
const COOLPIX_CA_UNIT: f64 = 1. / 3792.;

/// A Coolpix NRW's tables (P950, P1000, P1100): record 2, after "0100" and the
/// header bytes 3 3 17 1, holds 17 rationals, the distortion in percent at radii
/// spread evenly from the centre to the default crop's corner (green's source radius
/// over the output radius is 1 + d/100, through a natural spline); record 3, after
/// 3 34 1 0, red's and then blue's displacement from green at the same radii, 17
/// each. The A1000's record 2 has another header and is not read.
fn coolpix_tables(
    distortion: &[u8],
    ca: &[u8],
    m: &Metadata,
    rational: &dyn Fn(&[u8], usize) -> Option<f64>,
) -> Option<LensCorrection> {
    let table = |b: &[u8], header: [u8; 4], n: usize| -> Option<Vec<f64>> {
        if b.get(4..8)? != header {
            return None;
        }
        (0..n).map(|i| rational(b, 8 + 8 * i)).collect()
    };
    let d: Vec<f64> = table(distortion, [3, 3, 17, 1], 17)?
        .iter()
        .map(|d| 1. + d / 100.)
        .collect();
    let ca = table(ca, [3, 34, 1, 0], 34);
    let (w, h) = match (m.crop_width, m.crop_height) {
        (0, _) | (_, 0) => (m.width, m.height),
        size => size,
    };
    let unit = f64::from(w).hypot(f64::from(h)) * 0.5;
    let half = f64::from(m.width).hypot(f64::from(m.height)) * 0.5;
    if !(half > 0. && unit > 0.) {
        return None;
    }
    let curve = |f: &dyn Fn(f64) -> f64| {
        let values = knots(1.)
            .into_iter()
            .map(|r| f(f64::from(r) * half / unit) as f32);
        Radial::new(knots(1.), values.collect())
    };
    let ratio = spline(&d);
    // Scaled, as Adobe's warp is, so the widest ratio out to the corner is 1.
    let widest = (0..=64)
        .map(|i| ratio(f64::from(i) / 64.))
        .fold(1., f64::max);
    let changes = |r: &Radial, by: f32| r.values.iter().any(|v| (v - 1.).abs() > by);
    let distortion = curve(&|q| ratio(q) / widest).filter(|r| changes(r, 1e-6));
    // A displacement at green's source radius, as a scale of it at the output radius.
    let chromatic = ca
        .and_then(|ca| {
            let plane = |t: &[f64]| {
                let shift = spline(t);
                curve(&|q| {
                    let q = q.max(1. / 64.);
                    1. + shift(q) * COOLPIX_CA_UNIT / (q * ratio(q))
                })
            };
            Some([plane(&ca[..17])?, plane(&ca[17..])?])
        })
        .filter(|[a, b]| changes(a, 1e-7) || changes(b, 1e-7));
    Some(LensCorrection {
        source: "Nikon built-in".into(),
        // Adobe writes them as its DNGs' opcodes, which always apply.
        default_on: true,
        vignetting: None,
        distortion,
        chromatic,
        ..Default::default()
    })
}

/// Lenses Adobe has its own profiles for (LensType 1–3: NX 30mm F2, 18-55mm and
/// 50-200mm), compacts it has profiles for, and the first NX bodies, whose data
/// Adobe does not use at all.
const SAMSUNG_UNUSED_LENSES: [u32; 3] = [1, 2, 3];
const SAMSUNG_UNUSED_MODELS: [&str; 6] = ["EX1", "WB2000", "NX5", "NX10", "NX100", "NX11"];

fn samsung(path: &Path, t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let model = String::from_utf8_lossy(&t.raw(ifd0.get(&0x0110)?)?)
        .trim_end_matches(['\0', ' '])
        .to_owned();
    if SAMSUNG_UNUSED_MODELS.contains(&model.as_str()) {
        return None;
    }
    let exif = t.ifd(t.offset(ifd0.get(&0x8769)?)?)?;
    let at = t.u32(&exif.get(&0x927c)?.value) as u64;
    // A directory at the note's start, offsets counting from there.
    let mut n = Tiff::headless(File::open(path).ok()?, at, t.little);
    let note = n.ifd(0)?;
    let lens = note
        .get(&0xa003)
        .and_then(|e| n.numbers(e)?.first().copied());
    if lens.is_some_and(|l| SAMSUNG_UNUSED_LENSES.contains(&(l as u32))) {
        return None;
    }
    // Each value is offset by one of the EncryptionKey's eleven (0xA020).
    let key = note
        .get(&0xa020)
        .filter(|e| e.kind == kind::LONG && e.count == 11)?;
    let key: Vec<u32> = n
        .raw(key)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| n.u32(c))
        .collect();
    let distortion = note
        .get(&0xa050)
        .filter(|e| e.kind == kind::SLONG && e.count == 8)
        .and_then(|e| n.raw(e))
        .map(|b| -> Vec<i32> {
            (b.as_chunks::<4>().0.iter().enumerate())
                .map(|(i, c)| (n.u32(c).wrapping_add(key[i % 11])) as i32)
                .collect()
        });
    // After the count, which is not offset.
    let ca = note
        .get(&0xa051)
        .filter(|e| e.kind == kind::SHORT && e.count == 22)
        .and_then(|e| n.raw(e))
        .map(|b| -> Vec<f64> {
            (b.as_chunks::<2>().0.iter().enumerate())
                .map(|(i, c)| {
                    let k = if i == 0 { 0 } else { key[(i - 1) % 11] };
                    f64::from(n.u16(c).wrapping_sub(k as u16))
                })
                .collect()
        })
        .filter(|v| v[0] == 7.);
    // Green's source radius over the output radius r, in pixels: c0 + c1 r² + c2 r⁴ +
    // c3 r⁶, each c a mantissa and a power of ten. A zero c0 is no model.
    let c: Option<[f64; 4]> = distortion
        .map(|d| std::array::from_fn(|j| f64::from(d[2 * j]) * 10f64.powi(d[2 * j + 1])))
        .filter(|c| c[0] > 0. && c.iter().all(|x| x.is_finite()));
    let green = |r: f64| {
        let Some(c) = c else { return Some(r) };
        let r2 = r * r;
        Some(r * (c[0] + r2 * (c[1] + r2 * (c[2] + r2 * c[3]))))
    };
    // Adobe uses the first six radii and values, which hold one value per channel on
    // every file surveyed (blue's seventh often differs). It applies half of each
    // change, and on the EX2F three quarters of red's and a quarter of blue's.
    let ca = ca.filter(|v| v[1] > 0. && v[1..=6].is_sorted_by(|a, b| a < b));
    let share = if model == "EX2F" {
        [0.75, 0.25]
    } else {
        [0.5, 0.5]
    };
    let scale = |at: usize, g: f64| {
        let Some(v) = &ca else { return 1. };
        let (knots, values) = (&v[1..7], &v[at..at + 6]);
        let i = knots.iter().position(|k| g < *k).unwrap_or(6);
        let value = match i {
            0 => values[0],
            6 => values[5],
            i => {
                let t = (g - knots[i - 1]) / (knots[i] - knots[i - 1]);
                values[i - 1] + t * (values[i] - values[i - 1])
            }
        };
        1. + share[usize::from(at == 15)] * (value / 10_000. - 1.)
    };
    let (red, blue) = (|g| scale(8, g), |g| scale(15, g));
    let chromatic = ca.is_some().then_some([&red as &dyn Fn(f64) -> f64, &blue]);
    // Adobe's warps are centred on the default crop (NX1000: 7 px right of and 4 px
    // below the active area's centre).
    let centre = crop_centre(m);
    let (distortion, chromatic) = maker_model(m, 1., centre, &green, chromatic);
    Some(LensCorrection {
        source: "Samsung built-in".into(),
        // Adobe writes it into its DNGs as their warp, which always applies.
        default_on: true,
        vignetting: None,
        distortion,
        chromatic,
        chromatic_at_source: false,
        gain_map: None,
        centre,
    })
}

/// Canon's metadata box in a CR3's `moov`.
const CANON_UUID: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
/// The most of a CR3 read for its lens tables at once: its `moov` is some 50 kB, its
/// timed metadata some 120 kB.
const CR3_MAX: u64 = 4 << 20;
/// The bodies whose CR3 lens tables Adobe applies, with the width in pixels of their
/// full-frame image, 36 mm. DNG Converter 18.7 uses the tables of no other body, not
/// even of an R50 V, which stores the same record.
const CANON_BODIES: [(&str, f64); 2] =
    [("Canon EOS R5m2", 8192.), ("Canon EOS R6 Mark III", 6960.)];
/// The unit of Canon's knots in millimetres: Adobe's piecewise vignetting puts them at
/// this many on both bodies, to 0.02%.
const CANON_KNOT_MM: f64 = 0.005314;
/// Adobe fits Canon's distortion table with its values limited to this.
const CANON_DISTORTION_MAX: f64 = 1.0275;

/// The ISO base media boxes in `b`: their kind and body.
fn boxes(b: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let head = b.get(at..at.checked_add(8)?)?;
        let (start, len) = match u32::from_be_bytes(head[..4].try_into().ok()?) {
            0 => (at + 8, b.len() - at),
            1 => {
                let large = b.get(at + 8..at + 16)?.try_into().ok()?;
                (at + 16, usize::try_from(u64::from_be_bytes(large)).ok()?)
            }
            n => (at + 8, n as usize),
        };
        let body = b.get(start..at.checked_add(len)?)?;
        at += len;
        Some((&head[4..8], body))
    })
}

/// The body of the first box of `kind` in `b`.
fn child<'a>(b: &'a [u8], kind: &[u8]) -> Option<&'a [u8]> {
    boxes(b).find(|(k, _)| *k == kind).map(|(_, body)| body)
}

/// `len` bytes of `f` from `at`, at most `CR3_MAX`.
fn read_at(f: &mut File, at: u64, len: u64) -> Option<Vec<u8>> {
    if len > CR3_MAX {
        return None;
    }
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut b = vec![0; usize::try_from(len).ok()?];
    f.read_exact(&mut b).ok()?;
    Some(b)
}

/// The body of a CR3's `moov`, among its first top-level boxes.
fn cr3_moov(f: &mut File) -> Option<Vec<u8>> {
    let end = f.seek(SeekFrom::End(0)).ok()?;
    let mut at = 0;
    for _ in 0..16 {
        let h = read_at(f, at, 16.min(end.checked_sub(at)?))?;
        let (head, len) = match u32::from_be_bytes(h.get(..4)?.try_into().ok()?) {
            0 => (8, end - at),
            1 => (16, u64::from_be_bytes(h.get(8..16)?.try_into().ok()?)),
            n => (8, u64::from(n)),
        };
        if len < head || at.checked_add(len)? > end {
            return None;
        }
        if &h[4..8] == b"moov" {
            return read_at(f, at + head, len - head);
        }
        at += len;
    }
    None
}

/// Where a track's one sample is, `(offset, length)`, when it is timed metadata.
fn ctmd_sample(trak: &[u8]) -> Option<(u64, u64)> {
    let stbl = child(child(child(trak, b"mdia")?, b"minf")?, b"stbl")?;
    // A full box's version and flags, the entry count, then the first entry.
    if child(stbl, b"stsd")?.get(12..16)? != b"CTMD" {
        return None;
    }
    let be32 = |b: &[u8], at: usize| Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let stsz = child(stbl, b"stsz")?;
    let len = match be32(stsz, 4)? {
        0 => be32(stsz, 12)?,
        n => n,
    };
    let at = match child(stbl, b"co64") {
        Some(c) => u64::from_be_bytes(c.get(8..16)?.try_into().ok()?),
        None => u64::from(be32(child(stbl, b"stco")?, 8)?),
    };
    Some((at, u64::from(len)))
}

fn canon(f: &mut File, m: &Metadata) -> Option<LensCorrection> {
    let moov = cr3_moov(f)?;
    let (mut model, mut sample) = (None, None);
    for (kind, body) in boxes(&moov) {
        match kind {
            b"uuid" if body.starts_with(&CANON_UUID) => {
                // IFD0 of the TIFF in CMT1.
                let mut t = Tiff::open(Cursor::new(child(&body[16..], b"CMT1")?), 0)?;
                let ifd0 = t.ifd(t.first)?;
                let name = t.raw(ifd0.get(&0x0110)?)?;
                model = Some(
                    String::from_utf8_lossy(&name)
                        .trim_end_matches('\0')
                        .to_owned(),
                );
            }
            b"trak" => sample = sample.or_else(|| ctmd_sample(body)),
            _ => {}
        }
    }
    let model = model?;
    let width = CANON_BODIES.iter().find(|(name, _)| *name == model)?.1;
    let (at, len) = sample?;
    let sample = read_at(f, at, len)?;
    // Records: a little-endian length and kind, six more bytes, the data.
    let mut at = 0usize;
    let tables = std::iter::from_fn(|| {
        let len = u32::from_le_bytes(sample.get(at..at + 4)?.try_into().ok()?) as usize;
        let r = sample.get(at..at.checked_add(len)?).filter(|_| len >= 12)?;
        at += len;
        Some((u16::from_le_bytes([r[4], r[5]]), &r[12..]))
    })
    .find(|(kind, _)| *kind == 14)?
    .1;
    canon_tables(tables, width * CANON_KNOT_MM / 36., m)
}

/// Record 14 of a CR3's timed metadata: three counts of knots, then seven arrays of
/// as many slots each: vignetting and its knots, red, blue and their knots,
/// distortion and its knots. `unit` is a knot in pixels.
fn canon_tables(t: &[u8], unit: f64, m: &Metadata) -> Option<LensCorrection> {
    let v: Vec<f64> = (t.as_chunks::<2>().0.iter())
        .map(|c| f64::from(u16::from_le_bytes(*c)))
        .collect();
    let (counts, arrays) = v.split_at_checked(3)?;
    let slots = arrays.len() / 7;
    if slots < 2 || arrays.len() != 7 * slots {
        return None;
    }
    let half = f64::from(m.width).hypot(f64::from(m.height)) * 0.5;
    if !(half > 0. && unit > 0.) {
        return None;
    }
    // The `n` tables from array `first` and the knots after them, from the centre out.
    let group = |count: f64, first: usize, n: usize| {
        let count = count as usize;
        if !(2..=slots).contains(&count) {
            return None;
        }
        let array = |i: usize| &arrays[i * slots..i * slots + count];
        let knots = array(first + n);
        (knots[0] == 0. && knots.is_sorted_by(|a, b| a < b))
            .then(|| (knots, (first..first + n).map(array).collect::<Vec<_>>()))
    };
    let radii = |knots: &[f64]| knots.iter().map(|k| (k * unit / half) as f32).collect();
    let changes = |r: &Radial, by: f32| r.values.iter().any(|v| (v - 1.).abs() > by);
    // The share of light kept, in 2^-13, straight lines between the knots as in Adobe's
    // piecewise profile. The gain restores it at more radii in each interval, as many as
    // Radial holds (two more for 17 knots): the RF135mm F1.8's falls to 0.24, and lines
    // through the gains alone would be 0.0027 off between the knots.
    let vignetting = group(counts[0], 0, 1)
        .filter(|(_, t)| t[0].iter().all(|v| *v > 0.))
        .and_then(|(k, t)| {
            let parts = (63 / (k.len() - 1)).min(3);
            let lines = k.windows(2).zip(t[0].windows(2)).flat_map(|(k, v)| {
                (0..parts).map(move |i| {
                    let f = i as f64 / parts as f64;
                    (k[0] + (k[1] - k[0]) * f, v[0] + (v[1] - v[0]) * f)
                })
            });
            let (at, kept): (Vec<f64>, Vec<f64>) =
                lines.chain([(k[k.len() - 1], t[0][k.len() - 1])]).unzip();
            Radial::new(
                radii(&at),
                kept.iter().map(|v| (8192. / v) as f32).collect(),
            )
        })
        .filter(|r| changes(r, 1e-4));
    // Red's and blue's scale relative to green, in 2^-23 from 2^14.
    let chromatic = group(counts[1], 2, 2)
        .and_then(|(k, t)| {
            let scale = |t: &[f64]| {
                t.iter()
                    .map(|v| (1. + (v - 16384.) / 8_388_608.) as f32)
                    .collect()
            };
            Some([
                Radial::new(radii(k), scale(t[0]))?,
                Radial::new(radii(k), scale(t[1]))?,
            ])
        })
        .filter(|[a, b]| changes(a, 1e-7) || changes(b, 1e-7));
    let distortion = group(counts[2], 5, 1)
        .and_then(|(k, t)| canon_distortion(k, t[0], unit, half, m))
        .filter(|r| changes(r, 1e-6));
    Some(LensCorrection {
        source: "Canon built-in".into(),
        // A "Camera Settings" profile in Adobe's DNGs, which Enable Profile Corrections
        // applies (Camera Raw 18.7 renders of an R6 Mark III photo).
        default_on: false,
        vignetting,
        distortion,
        chromatic,
        ..Default::default()
    })
}

/// Canon's distortion table, green's source radius over the output radius in 2^-14 at
/// each knot (`at`, in knots), as Adobe turns it into its profile: a least-squares fit of 1, r², r⁴
/// and r⁶ to the values past the centre, residuals weighted by the radius, the
/// knots' radii scaled by the values' mean to the power 0.75; of which the change
/// from 1 over the constant term, at 80%, is the profile's ratio, scaled so the
/// default crop's corner stays in place. This matches the profiles DNG Converter
/// 18.7 writes for edited copies of an R6 Mark III file (single knots raised or
/// lowered by 1%, smooth tables, the real one scaled by -1 to 1.5) to within 0.15 px.
fn canon_distortion(
    at: &[f64],
    values: &[f64],
    unit: f64,
    half: f64,
    m: &Metadata,
) -> Option<Radial> {
    let d: Vec<f64> = values[1..]
        .iter()
        .map(|v| (v / 16384.).min(CANON_DISTORTION_MAX))
        .collect();
    if d.len() < 4 {
        return None;
    }
    let last = at[at.len() - 1];
    let spread = (d.iter().sum::<f64>() / d.len() as f64).powf(0.75);
    let mut normal = [[0.; 5]; 4];
    for (k, y) in at[1..].iter().zip(&d) {
        let t2 = (k / last * spread).powi(2);
        let row = [1., t2, t2 * t2, t2 * t2 * t2];
        for (j, n) in normal.iter_mut().enumerate() {
            for i in 0..4 {
                n[i] += t2 * row[j] * row[i];
            }
            n[4] += t2 * row[j] * y;
        }
    }
    let a = solve4(normal).filter(|a| a[0] > 0.)?;
    let c = [1, 2, 3].map(|i| 0.8 * a[i] / a[0]);
    let ratio = |px: f64| {
        let t2 = (px / (last * unit)).powi(2);
        1. + t2 * (c[0] + t2 * (c[1] + t2 * c[2]))
    };
    let (w, h) = match (m.crop_width, m.crop_height) {
        (0, _) | (_, 0) => (m.width, m.height),
        size => size,
    };
    // Adobe's ScaleFactor keeps the corner about 2·10⁻⁵ further in (0.1 px).
    let scale = 1. / ratio(f64::from(w).hypot(f64::from(h)) * 0.5);
    // Radial's most knots: with 33, an RF135mm F1.8's curve, which bends sharply near
    // the corner, is 0.08 px off between them.
    let radii: Vec<f32> = (0..64).map(|i| i as f32 / 63.).collect();
    let values: Vec<f32> = (radii.iter())
        .map(|r| (scale * ratio(f64::from(*r) * half)) as f32)
        .collect();
    // The source radius grows with the output radius.
    let sources: Vec<f32> = radii.iter().zip(&values).map(|(r, v)| r * v).collect();
    (sources.windows(2).all(|p| p[0] < p[1])).then(|| Radial::new(radii, values))?
}

#[cfg(test)]
mod tests {
    use crate::camera_data::Metadata;
    use crate::optics::LensCorrection;
    use std::path::Path;
    /// The correction of a file whose radii need no image size (Fujifilm, Sony).
    fn read(p: &Path) -> Option<LensCorrection> {
        super::read(p, &Metadata::default())
    }
    /// A Sony file's always-applied correction and the stand-in for a profile.
    fn sony(p: &Path) -> (Option<LensCorrection>, LensCorrection) {
        let b = super::read_builtin(p, &Metadata::default());
        (b.lens, b.stand_in.unwrap())
    }
    /// (tag, type, count, data)
    type Entry = (u16, u16, u32, Vec<u8>);
    /// Little-endian TIFF with the given directories, each pointed to by the previous one.
    fn tiff(dirs: &[Vec<Entry>]) -> Vec<u8> {
        // Directories are laid out in order; entry data follows each directory.
        let mut out = b"II*\0\x08\0\0\0".to_vec();
        for (d, entries) in dirs.iter().enumerate() {
            let start = out.len();
            let data_start = start + 2 + entries.len() * 12 + 4;
            let mut data: Vec<u8> = Vec::new();
            out.extend((entries.len() as u16).to_le_bytes());
            for (tag, kind, count, bytes) in entries {
                out.extend(tag.to_le_bytes());
                out.extend(kind.to_le_bytes());
                out.extend(count.to_le_bytes());
                if bytes.len() <= 4 {
                    let mut v = bytes.clone();
                    v.resize(4, 0);
                    out.extend(v);
                } else {
                    out.extend(((data_start + data.len()) as u32).to_le_bytes());
                    data.extend(bytes);
                }
            }
            out.extend(0u32.to_le_bytes());
            out.extend(data);
            // A pointer entry value of 0 is patched to the next directory.
            if d + 1 < dirs.len() {
                let next = out.len() as u32;
                let value = start + 2 + 8;
                out[value..value + 4].copy_from_slice(&next.to_le_bytes());
            }
        }
        out
    }
    fn shorts(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn rationals(v: &[f64]) -> Vec<u8> {
        v.iter()
            .flat_map(|x| {
                let n = (*x * 1_000_000.).round() as i32;
                [n.to_le_bytes(), 1_000_000i32.to_le_bytes()].concat()
            })
            .collect()
    }
    #[test]
    fn reads_sony_subifd_tables() {
        let mut vig = vec![16i16];
        vig.extend((0..16).map(|i| i * 800));
        let mut dist = vec![16i16];
        dist.extend((0..16).map(|i| i * 10));
        let mut ca = vec![32i16];
        ca.extend(std::iter::repeat_n(-384, 16));
        ca.extend(std::iter::repeat_n(0, 16));
        let bytes = tiff(&[
            vec![(0x14a, 4, 1, vec![0; 4])],
            vec![
                (0x7032, 8, 17, shorts(&vig)),
                (0x7035, 8, 33, shorts(&ca)),
                (0x7037, 8, 17, shorts(&dist)),
            ],
        ]);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.arw");
        std::fs::write(&p, bytes).unwrap();
        let (lens, c) = sony(&p);
        // Lightroom applies the lateral CA, not this body's distortion or vignetting.
        let lens = lens.unwrap();
        assert!(lens.default_on && lens.vignetting.is_none() && lens.distortion.is_none());
        assert_eq!(lens.chromatic, c.chromatic);
        let v = c.vignetting.as_ref().unwrap();
        assert!((v.eval(0.) - 1.).abs() < 1e-6);
        let corner = 2f32.powf(2f32.powf(12000. / 8192. - 1.) - 0.5);
        assert!((v.eval(1.) - corner).abs() < 1e-5);
        assert!((c.distortion.as_ref().unwrap().eval(1.) - (1. + 150. / 16384.)).abs() < 1e-6);
        let [red, blue] = c.chromatic.as_ref().unwrap();
        assert!((red.eval(0.5) - (1. - 384. / 2_097_152.)).abs() < 1e-7);
        assert_eq!(blue.eval(0.5), 1.);
    }
    fn sony_file(
        dir: &std::path::Path,
        vig: &[i16],
        ca: &[i16],
        dist: &[i16],
    ) -> std::path::PathBuf {
        sony_model_file(dir, "ZV-1", vig, ca, dist)
    }
    fn sony_model_file(
        dir: &std::path::Path,
        model: &str,
        vig: &[i16],
        ca: &[i16],
        dist: &[i16],
    ) -> std::path::PathBuf {
        let entry = |tag, v: &[i16]| (tag, 8, v.len() as u32, shorts(v));
        let model = [model.as_bytes(), b"\0"].concat();
        let bytes = tiff(&[
            vec![
                (0x14a, 4, 1, vec![0; 4]),
                (0x0110, 2, model.len() as u32, model),
            ],
            vec![entry(0x7032, vig), entry(0x7035, ca), entry(0x7037, dist)],
        ]);
        let p = dir.join("a.arw");
        std::fs::write(&p, bytes).unwrap();
        p
    }
    /// A ZV-1 stores fewer knots than its fixed 16 (CA: 32) slots, padded with zeros (#403).
    const ZV1_VIG: [i16; 17] = [
        16, 0, 0, 0, 15, 63, 193, 413, 715, 1107, 1576, 2126, 2749, 3443, 4523, 5509, 6639,
    ];
    const ZV1_CA: [i16; 33] = [
        22, 0, 256, 384, 128, 128, 256, 384, 640, 896, 1152, 1408, 0, 1280, 1664, 1664, 1280, 896,
        512, 256, 128, 0, 256, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    const ZV1_DIST: [i16; 17] = [
        11, 1265, 1218, 1083, 860, 562, 209, -179, -580, -972, -1353, -1725, 0, 0, 0, 0, 0,
    ];
    #[test]
    fn reads_sony_tables_padded_to_fixed_slots() {
        let d = tempfile::tempdir().unwrap();
        let (lens, c) = sony(&sony_file(d.path(), &ZV1_VIG, &ZV1_CA, &ZV1_DIST));
        let dist = c.distortion.as_ref().unwrap();
        // The 11 knots, through which a spline is sampled at 33.
        for (i, v) in [1265., 1218., 1083., 860., 562., 209.]
            .into_iter()
            .enumerate()
        {
            let at = dist.eval(i as f32 / 10.);
            assert!((at - (1. + v / 16384.)).abs() < 1e-4, "{i}: {at}");
        }
        assert!((dist.eval(1.) - (1. - 1725. / 16384.)).abs() < 1e-6);
        // A compact: Lightroom applies its distortion too, not its vignetting.
        let lens = lens.unwrap();
        assert_eq!(lens.distortion, c.distortion);
        assert!(lens.vignetting.is_none());
        let [red, blue] = c.chromatic.as_ref().unwrap();
        assert_eq!(red.knots.len(), 11);
        assert!((red.eval(1.) - (1. + 1408. / 2_097_152.)).abs() < 1e-7);
        assert!((blue.eval(0.1) - (1. + 1280. / 2_097_152.)).abs() < 1e-7);
        assert!((blue.eval(1.) - (1. + 256. / 2_097_152.)).abs() < 1e-7);
        assert_eq!(c.vignetting.as_ref().unwrap().knots.len(), 16);
    }
    /// Adobe writes the stored distortion of Sony's fixed-lens compacts as an opcode, but
    /// neither an interchangeable-lens body's nor the RX1 series'.
    #[test]
    fn sony_distortion_applies_on_compacts_only() {
        let d = tempfile::tempdir().unwrap();
        for (model, applies) in [
            ("DSC-RX100M7", true),
            ("DSC-RX10M4", true),
            ("DSC-HX99", true),
            ("ZV-1M2", true),
            ("DSC-RX1RM3", false),
            ("DSC-RX1", false),
            ("ILCE-7M4", false),
            ("ZV-E10", false),
        ] {
            let p = sony_model_file(d.path(), model, &ZV1_VIG, &ZV1_CA, &ZV1_DIST);
            let (lens, c) = sony(&p);
            let lens = lens.unwrap();
            assert_eq!(lens.distortion.is_some(), applies, "{model}");
            assert!(
                lens.chromatic.is_some() && lens.vignetting.is_none(),
                "{model}"
            );
            assert!(c.distortion.is_some() && c.vignetting.is_some(), "{model}");
        }
    }
    #[test]
    fn rejects_malformed_sony_tables() {
        fn with<const N: usize>(mut v: [i16; N], i: usize, x: i16) -> [i16; N] {
            v[i] = x;
            v
        }
        let d = tempfile::tempdir().unwrap();
        for (name, dist, ca) in [
            (
                "data past the count",
                with(ZV1_DIST, 16, 1),
                with(ZV1_CA, 32, 1),
            ),
            (
                "count past the stored values",
                with(ZV1_DIST, 0, 17),
                with(ZV1_CA, 0, 34),
            ),
            ("a single knot", with(ZV1_DIST, 0, 1), with(ZV1_CA, 0, 2)),
        ] {
            let (_, c) = sony(&sony_file(d.path(), &ZV1_VIG, &ca, &dist));
            assert!(c.distortion.is_none() && c.chromatic.is_none(), "{name}");
        }
        let odd = with(ZV1_CA, 0, 21);
        let (_, c) = sony(&sony_file(d.path(), &ZV1_VIG, &odd, &ZV1_DIST));
        assert!(c.chromatic.is_none(), "odd CA count");
    }
    #[test]
    fn reads_fujifilm_raf_tables() {
        let knots: Vec<f64> = (0..=10).map(|i| i as f64 / 10.).collect();
        let mut vig = vec![327.7];
        vig.extend(&knots);
        vig.extend([
            100., 99.5, 98.8, 97.5, 95.4, 92.2, 87.7, 82.4, 75.8, 68.3, 60.4,
        ]);
        let mut dist = vec![327.7];
        dist.extend(&knots);
        dist.extend(std::iter::repeat_n(0., 11));
        let fuji = tiff(&[
            vec![(0xf000, 13, 1, vec![0; 4])],
            vec![
                (0xf00b, 10, 23, rationals(&dist)),
                (0xf010, 10, 23, rationals(&vig)),
            ],
        ]);
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(128, 0);
        raf[100..104].copy_from_slice(&128u32.to_be_bytes());
        raf.extend(fuji);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.raf");
        std::fs::write(&p, raf).unwrap();
        let c = read(&p).unwrap();
        assert!(c.default_on);
        assert!(c.distortion.is_none(), "identity distortion is omitted");
        let v = c.vignetting.as_ref().unwrap();
        let gain = |p: f32| (100. / p).powf(super::FUJI_VIGNETTE_STRENGTH);
        assert!((v.eval(1.) - gain(60.4)).abs() < 1e-4);
        assert!((v.eval(0.85) - (gain(75.8) + gain(68.3)) / 2.).abs() < 1e-4);
        // A pincushion correction is scaled so its corner samples the corner, as Adobe
        // frames it, since the table has no fill scale.
        let mut dist = vec![327.7];
        dist.extend(&knots);
        dist.extend((0..=10).map(|i| 0.98 * (i as f64 / 10.).powi(2)));
        let fuji = tiff(&[
            vec![(0xf000, 13, 1, vec![0; 4])],
            vec![(0xf00b, 10, 23, rationals(&dist))],
        ]);
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(128, 0);
        raf[100..104].copy_from_slice(&128u32.to_be_bytes());
        raf.extend(fuji);
        std::fs::write(&p, raf).unwrap();
        let d = read(&p).unwrap().distortion.unwrap();
        assert!((d.eval(1.) - 1.).abs() < 1e-6);
        assert!((d.eval(0.) - 1. / 1.0098).abs() < 1e-5, "{}", d.eval(0.));
    }
    /// Little-endian directory of `entries` (tag, type, count, data) at `at`, as a
    /// file's or maker note's offsets count; the data follows it.
    fn directory(at: usize, entries: &[Entry]) -> Vec<u8> {
        let mut data_at = at + 2 + entries.len() * 12 + 4;
        let (mut out, mut data): (Vec<u8>, Vec<u8>) =
            ((entries.len() as u16).to_le_bytes().to_vec(), vec![]);
        for (tag, kind, count, bytes) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                out.extend(bytes.iter().chain(&[0; 4]).take(4));
            } else {
                out.extend((data_at as u32).to_le_bytes());
                data.extend(bytes);
                data_at += bytes.len();
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(data);
        out
    }
    /// An ORF whose maker note's ImageProcessing directory holds `processing`.
    fn orf(processing: &[Entry]) -> Vec<u8> {
        orf_with(b"OLYMPUS\0II\x03\0", processing)
    }
    /// An ORF whose maker note starts with `header`, ending in a byte order mark and
    /// two version bytes.
    fn orf_with(header: &[u8], processing: &[Entry]) -> Vec<u8> {
        let main = header.len();
        let mut note = header.to_vec();
        let at = (main + 18) as u32;
        note.extend(directory(
            main,
            &[(0x2040, 13, 1, at.to_le_bytes().to_vec())],
        ));
        note.extend(directory(at as usize, processing));
        let mut out = b"IIRO\x08\0\0\0".to_vec();
        out.extend(directory(
            8,
            &[(0x8769, 4, 1, 26u32.to_le_bytes().to_vec())],
        ));
        out.extend(directory(
            26,
            &[(0x927c, 7, note.len() as u32, note.clone())],
        ));
        out
    }
    fn hex(s: &str) -> Vec<u8> {
        (s.split_whitespace())
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }
    /// Source radius in pixels at output radius `px` from Adobe's WarpRectilinear
    /// coefficients, whose radius 1 is `unit` pixels.
    fn adobe(k: [f64; 4], unit: f64, px: f64) -> f64 {
        let r2 = (px / unit).powi(2);
        px * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3])))
    }
    /// Decoded image size and default crop.
    fn frame(size: [u32; 2], crop: [u32; 4]) -> Metadata {
        let [crop_left, crop_top, crop_width, crop_height] = crop;
        Metadata {
            width: size[0],
            height: size[1],
            crop_left,
            crop_top,
            crop_width,
            crop_height,
            ..Default::default()
        }
    }
    /// Largest difference in pixels, out to the default crop's corner farthest from the
    /// optical centre, between the source radii of `c` and Adobe's warp.
    fn largest_difference(
        c: &LensCorrection,
        m: &Metadata,
        adobe: &dyn Fn(usize, f64) -> f64,
    ) -> f64 {
        let half = f64::from(m.width).hypot(f64::from(m.height)) / 2.;
        let [cx, cy] = c
            .centre_in([m.width as f32, m.height as f32])
            .map(f64::from);
        let (l, t) = (f64::from(m.crop_left), f64::from(m.crop_top));
        let (r, b) = (l + f64::from(m.crop_width), t + f64::from(m.crop_height));
        let corner = (cx - l).max(r - cx).hypot((cy - t).max(b - cy));
        (0..=200)
            .flat_map(|i| {
                let px = corner * f64::from(i) / 200.;
                let scale = c.radial_scale_with((px / half) as f32, 1.);
                (0..3).map(move |ch| (px * f64::from(scale[ch]) - adobe(ch, px)).abs())
            })
            .fold(0., f64::max)
    }
    /// PA080192.ORF (raw.pixls.us, E-M10 Mark III, M.17mm F1.8): its tags, against the
    /// warp Adobe DNG Converter 18.7 writes for it (radius 1 at the 4640 × 3472 image's
    /// corner farthest from its centre, (2312, 1736)).
    #[test]
    fn reads_olympus_distortion_and_chromatic_aberration_as_adobe_does() {
        let distortion = hex("97 f4 9a bd 66 06 cb bb b3 e4 b7 3b 00 00 80 3f");
        let ca = hex("00 98 2d 3a 00 20 94 b9 00 80 b9 b9 00 58 20 ba 00 48 80 3a 00 a0 64 39");
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.orf");
        std::fs::write(&p, orf(&[(0x150a, 11, 4, distortion), (0x150c, 11, 6, ca)])).unwrap();
        let m = frame([4640, 3472], [8, 8, 4608, 3456]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.default_on && c.vignetting.is_none());
        assert_eq!(c.source, "Olympus built-in");
        // The default crop's centre, as Adobe's warp has it: 8 px left of the image's.
        assert_eq!(c.centre_in([4640., 3472.]), [2312., 1736.]);
        let warp = [
            [
                1.000663065655637,
                -0.07727914192099739,
                -0.006663217130000866,
                0.005985386689871972,
            ],
            [
                1.000000000000001,
                -0.0769286992183596,
                -0.006405051999955553,
                0.0058986243274408824,
            ],
            [
                0.999387503381258,
                -0.07587444557829748,
                -0.006451880311619311,
                0.005871129977034196,
            ],
        ];
        let unit = 2328f64.hypot(1736.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
        // Without the correction the corner is 220 px out.
        assert!(adobe(warp[1], unit, 2880.) < 2880. - 200.);
    }
    /// OM System bodies start their note with a longer header.
    #[test]
    fn reads_om_system_notes() {
        let distortion = [-0.02f32, 0.01, -0.003, 1.]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.orf");
        let file = orf_with(b"OM SYSTEM\0\0\0II\x04\0", &[(0x150a, 11, 4, distortion)]);
        std::fs::write(&p, file).unwrap();
        let m = frame([5240, 3912], [8, 8, 5184, 3888]);
        let c = super::read(&p, &m).unwrap();
        assert_eq!(c.source, "OM System built-in");
        assert!(c.chromatic.is_none());
        // Radius 1 of the model is the default crop's half diagonal, 3240 px.
        let half = 5240f32.hypot(3912.) / 2.;
        let at = |px: f32| c.distortion.as_ref().unwrap().eval(px / half);
        let model = |x: f32| 1. + x * x * (-0.02 + x * x * (0.01 - 0.003 * x * x));
        assert!((at(3240.) - model(1.)).abs() < 1e-5);
        assert!((at(1620.) - model(0.5)).abs() < 1e-5);
    }
    #[test]
    fn olympus_note_without_corrections_has_none() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.orf");
        let zero = |n: usize| [0, 0, 0, 0].repeat(n);
        let one = [&zero(3)[..], &1f32.to_le_bytes()].concat();
        let m = frame([4640, 3472], [8, 8, 4608, 3456]);
        std::fs::write(&p, orf(&[(0x150a, 11, 4, one), (0x150c, 11, 6, zero(6))])).unwrap();
        assert!(super::read(&p, &m).is_none());
        std::fs::write(&p, orf(&[(0x0100, 3, 2, vec![0; 4])])).unwrap();
        assert!(super::read(&p, &m).is_none());
    }
    /// An RW2 with DistortionInfo `info`.
    fn rw2(info: &[i16]) -> Vec<u8> {
        rw2_with(Some(info), None)
    }
    /// An RW2 with DistortionInfo `info` and the lateral CA table `ca` (0x011B).
    fn rw2_with(info: Option<&[i16]>, ca: Option<&[i16]>) -> Vec<u8> {
        let mut out = b"IIU\0\x08\0\0\0".to_vec();
        let mut entries = vec![(0x010f, 2, 10, b"Panasonic\0".to_vec())];
        for (tag, v) in [(0x0119, info), (0x011b, ca)] {
            if let Some(v) = v {
                entries.push((tag, 7, 2 * v.len() as u32, shorts(v)));
            }
        }
        out.extend(directory(8, &entries));
        // At least as long as the header `read` reads.
        out.resize(out.len().max(128), 0);
        out
    }
    /// gx9_3_2.RW2 (raw.pixls.us, DC-GX9, LUMIX G 25/F1.7): its DistortionInfo, against
    /// the green plane of the warp Adobe DNG Converter 18.7 writes for it (radius 1 at
    /// the 5208 × 3904 image's corner farthest from (2600, 1952)).
    const GX9: [i16; 16] = [
        -24389, -25169, 545, 0, 652, 0, 211, 1, 447, 600, 434, -38, 3240, 1100, -30428, 8502,
    ];
    #[test]
    fn reads_panasonic_distortion_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        std::fs::write(&p, rw2(&GX9)).unwrap();
        let m = frame([5200, 3904], [8, 8, 5184, 3888]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.default_on && c.chromatic.is_none() && c.vignetting.is_none());
        assert_eq!(c.source, "Panasonic built-in");
        let green = [
            1.0000012404328427,
            -0.013760663120671701,
            -0.02015800015496616,
            0.004736213335729822,
        ];
        let unit = 2608f64.hypot(1952.);
        let off = largest_difference(&c, &m, &|_, px| adobe(green, unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
    }
    /// The DMC-GM5's DistortionInfo layout: a scale in value 5 (P1080764.RW2,
    /// raw.pixls.us, 3:2 of a 4:3 sensor), against the green plane of Adobe's warp.
    #[test]
    fn reads_panasonic_distortion_scale() {
        let gm5 = [
            25348, 2828, 62, 619, 0, -32, 0, -4095, 51, 628, 516, -19, 2870, 1128, -17663, -1541,
        ];
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        std::fs::write(&p, rw2(&gm5)).unwrap();
        let m = frame([4608, 3080], [8, 8, 4592, 3064]);
        let c = super::read(&p, &m).unwrap();
        let green = [
            0.9990234634717745,
            -0.0014475342840505334,
            9.97146343806321e-06,
            0.00045846619652323284,
        ];
        let unit = 2304f64.hypot(1540.);
        let off = largest_difference(&c, &m, &|_, px| adobe(green, unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
    }
    /// Largest difference in pixels, out to the default crop's corner, between red's
    /// and blue's displacement from green in `c` and in Adobe's warp, and Adobe's
    /// largest displacement.
    fn largest_ca_difference(
        c: &LensCorrection,
        m: &Metadata,
        adobe: &dyn Fn(usize, f64) -> f64,
    ) -> (f64, f64) {
        let half = f64::from(m.width).hypot(f64::from(m.height)) / 2.;
        let corner = f64::from(m.crop_width).hypot(f64::from(m.crop_height)) / 2.;
        (0..=200)
            .flat_map(|i| {
                let px = corner * f64::from(i) / 200.;
                let scale = c.radial_scale_with((px / half) as f32, 1.);
                [0, 2].map(|ch| {
                    let ours = px * f64::from(scale[ch] - scale[1]);
                    let theirs = adobe(ch, px) - adobe(1, px);
                    ((ours - theirs).abs(), theirs.abs())
                })
            })
            .fold((0., 0.), |(a, b), (x, y)| (a.max(x), b.max(y)))
    }
    /// Raw.pixls.us photos: their DistortionInfo and lateral CA tags, the decoded
    /// image's size and default crop, and the red, green and blue planes of the warp
    /// Adobe DNG Converter 18.7 writes for them, whose radius 1 is the farthest corner
    /// of the DNG's image from its optical centre.
    struct PanasonicCa {
        info: [i16; 16],
        ca: [i16; 32],
        frame: ([u32; 2], [u32; 4]),
        warp: [[f64; 4]; 3],
        /// The corner's offsets from the centre, in pixels.
        corner: [f64; 2],
    }
    /// gx9_3_2.RW2 (DC-GX9, LUMIX G 25/F1.7).
    const GX9_CA: PanasonicCa = PanasonicCa {
        info: GX9,
        ca: [
            -30167, -1805, 16569, 17920, 2730, 1024, 128, 21439, -246, 2184, -170, 3276, -473,
            18937, 256, 21168, 2184, 1092, 18608, 16640, -738, 0, -22016, -464, 2760, -464, 643,
            189, 17408, -2007, 17139, 15825,
        ],
        frame: ([5200, 3904], [8, 8, 5184, 3888]),
        warp: [
            [
                1.0004682282802129,
                -0.01451169102033667,
                -0.019598127970462897,
                0.004331191680153168,
            ],
            [
                1.0000012404328427,
                -0.013760663120671701,
                -0.02015800015496616,
                0.004736213335729822,
            ],
            [
                0.9998048878471808,
                -0.01311285159727582,
                -0.0202450377845782,
                0.004896631126171336,
            ],
        ],
        corner: [2608., 1952.],
    };
    #[test]
    fn reads_panasonic_chromatic_aberration_as_adobe_does() {
        // P1000403.RW2 (DMC-G80, SIGMA 60mm F2.8 DN): distortion correction off.
        let g80 = PanasonicCa {
            info: [
                8820, 0, 0, 0, 0, 0, 0, 0, 0, 0, 516, 0, 2870, 0, -12432, -15201,
            ],
            ca: [
                -2493, -15813, 1160, 12032, 2407, 256, 0, 14334, -633, 1408, 748, 2888, -230, -19,
                256, 1536, 1926, 963, 1376, 8320, 953, 96, 22912, -485, 10182, -485, -806, 361,
                512, 1175, -843, -17923,
            ],
            frame: ([4608, 3464], [8, 8, 4592, 3448]),
            warp: [
                [
                    1.0002378144655706,
                    2.9725538808200724e-05,
                    9.90484904583866e-06,
                    1.8099946004213052e-06,
                ],
                [1., 0., 0., 0.],
                [
                    0.9996276377798108,
                    -4.8152284437108506e-05,
                    3.178458018560093e-05,
                    -1.8260554076832136e-05,
                ],
            ],
            corner: [2304., 1732.],
        };
        // P1000416.RW2 (DC-TZ202): strong distortion, CA knots at 2/7, 4/7 and 6/7 of
        // the radius, and up to 3.9 px of CA. Adobe's polynomial departs from the
        // straight lines between the knots by up to 0.85 px here.
        let tz202 = PanasonicCa {
            info: [
                24343, 1775, 408, 0, 1460, 0, 631, 1, 6781, 189, 508, 587, 3288, 689, 24841, -25795,
            ],
            ca: [
                -18496, 12507, 4424, -25296, 2808, 384, -2560, 13819, 234, 832, -963, 3276, -586,
                -26642, 256, 8520, 1872, 936, -32280, 4416, -3385, 2112, 0, -859, -14384, -347,
                3085, -325, -22528, -8437, -18839, 17586,
            ],
            frame: ([5488, 3664], [8, 8, 5472, 3648]),
            warp: [
                [
                    0.9998364018762427,
                    -0.2003806248746034,
                    0.05753545636529678,
                    -0.00963193389561301,
                ],
                [
                    0.9991955093872854,
                    -0.1997130260581652,
                    0.057697840880385876,
                    -0.009660068321890813,
                ],
                [
                    0.9995507528793499,
                    -0.19942467462750102,
                    0.05834188670640916,
                    -0.009773256243380768,
                ],
            ],
            corner: [2744., 1832.],
        };
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        for (name, case) in [("GX9", &GX9_CA), ("G80", &g80), ("TZ202", &tz202)] {
            std::fs::write(&p, rw2_with(Some(&case.info), Some(&case.ca))).unwrap();
            let m = frame(case.frame.0, case.frame.1);
            let c = super::read(&p, &m).unwrap();
            assert!(c.chromatic.is_some(), "{name}");
            let unit = case.corner[0].hypot(case.corner[1]);
            let adobe = |ch: usize, px| adobe(case.warp[ch], unit, px);
            let (off, ca) = largest_ca_difference(&c, &m, &adobe);
            assert!(ca > 1., "{name}: Adobe corrects {ca} px");
            assert!(off < 0.1, "{name}: {off} px from Adobe's CA");
        }
        // The G80's lens needs no distortion correction, only the CA.
        std::fs::write(&p, rw2_with(Some(&g80.info), Some(&g80.ca))).unwrap();
        let c = super::read(&p, &frame(g80.frame.0, g80.frame.1)).unwrap();
        assert!(c.distortion.is_none());
    }
    /// Newer bodies, whose DNGs carry the distortion in an embedded profile, keep the CA
    /// in the warp, on the sensor's radius: P1002296.RW2 (DC-GH7, LEICA DG 12-35/F2.8 at
    /// 12mm).
    #[test]
    fn reads_panasonic_chromatic_aberration_on_the_sensor_radius() {
        let info = [
            18216, -26703, 241, 0, -399, 0, 124, 1, 4495, 436, 354, -1062, 3611, 936, -23617,
            -17367,
        ];
        let ca = [
            27551, 1983, 18, 15368, 3032, 32, -5888, 6654, -1268, 338, -546, 3638, -725, -1, 256,
            2064, 2426, 1213, 18128, 0, -1086, 384, -3584, -1268, -4230, -1268, -676, -159, -17152,
            -2148, -15798, -17015,
        ];
        let warp = [
            [
                1.000588699784562,
                -2.5632476810422133e-05,
                -0.00029661560319560465,
                -8.09482177484e-05,
            ],
            [1., 0., 0., 0.],
            [
                1.0001421369447667,
                1.0062062321125609e-05,
                0.0005116995869975938,
                -7.363913965363622e-05,
            ],
        ];
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        std::fs::write(&p, rw2_with(Some(&info), Some(&ca))).unwrap();
        let m = frame([5792, 4352], [8, 8, 5776, 4336]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.distortion.is_some());
        let half = 5792f64.hypot(4352.) / 2.;
        // Adobe's warp applies at the sensor radius green samples after distortion.
        let sensor = |px: f64| px * f64::from(c.radial_scale_with((px / half) as f32, 1.)[1]);
        let unit = 2896f64.hypot(2176.);
        let (off, most) =
            largest_ca_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, sensor(px)));
        assert!(most > 1., "Adobe corrects {most} px");
        assert!(off < 0.1, "{off} px from Adobe's CA");
    }
    /// Lenses without CA data store an empty table; malformed tables are not read.
    #[test]
    fn panasonic_chromatic_tables_without_data_are_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        let m = frame(GX9_CA.frame.0, GX9_CA.frame.1);
        let mut empty = [0; 32];
        // A DMC-G6's: only the values that change with each shot are set.
        [empty[0], empty[1], empty[2], empty[7], empty[13], empty[31]] =
            [-5508, 3276, 4624, -26724, 24178, 852];
        let mut unordered = GX9_CA.ca;
        unordered.swap(16, 17);
        let mut unknown = GX9_CA.ca;
        unknown[14] = 512;
        for (name, ca) in [
            ("empty", &empty[..]),
            ("unordered", &unordered[..]),
            ("unknown layout", &unknown[..]),
            ("short", &GX9_CA.ca[..31]),
        ] {
            std::fs::write(&p, rw2_with(Some(&GX9), Some(ca))).unwrap();
            let c = super::read(&p, &m).unwrap();
            assert!(c.chromatic.is_none() && c.distortion.is_some(), "{name}");
            std::fs::write(&p, rw2_with(None, Some(ca))).unwrap();
            assert!(super::read(&p, &m).is_none(), "{name}");
        }
        // Without DistortionInfo the CA is still read.
        std::fs::write(&p, rw2_with(None, Some(&GX9_CA.ca))).unwrap();
        let c = super::read(&p, &m).unwrap();
        assert!(c.chromatic.is_some() && c.distortion.is_none());
    }
    /// A model whose correction folds back on itself has no inverse to sample.
    #[test]
    fn panasonic_model_without_inverse_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        let mut folding = GX9;
        folding[11] = -32768;
        std::fs::write(&p, rw2(&folding)).unwrap();
        let m = frame([5200, 3904], [8, 8, 5184, 3888]);
        assert!(super::read(&p, &m).is_none());
    }
    #[test]
    fn panasonic_correction_switched_off_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        let mut off = GX9;
        off[7] = 0;
        std::fs::write(&p, rw2(&off)).unwrap();
        let m = frame([5200, 3904], [8, 8, 5184, 3888]);
        assert!(super::read(&p, &m).is_none());
    }
    /// A NEF whose raw SubIFD's NEFInfo holds lens-correction records 5 to 8.
    fn nef(records: [&[u8]; 4]) -> Vec<u8> {
        let entries: Vec<Entry> = (5..)
            .zip(records)
            .map(|(tag, r)| (tag, 7, r.len() as u32, r.to_vec()))
            .collect();
        let mut info = b"Nikon\0\x01\x01\0\0II*\0\x08\0\0\0".to_vec();
        info.extend(directory(8, &entries));
        tiff(&[
            vec![
                (0x14a, 4, 1, vec![0; 4]),
                (0x010f, 2, 18, b"NIKON CORPORATION\0".to_vec()),
            ],
            vec![(0xc7d5, 7, info.len() as u32, info)],
        ])
    }
    /// DSCN0173.NRW (raw.pixls.us, COOLPIX P1100): NEFInfo records 2 and 3.
    const P1100: [&str; 2] = [
        "30 31 30 30 03 03 11 01 00 00 00 00 a0 86 01 00 96 03 00 00 a0 86 01 00 58 0e 00 00
         a0 86 01 00 4c 20 00 00 a0 86 01 00 77 39 00 00 a0 86 01 00 e3 59 00 00 a0 86 01 00
         9d 81 00 00 a0 86 01 00 b2 b0 00 00 a0 86 01 00 33 e7 00 00 a0 86 01 00 33 25 01 00
         a0 86 01 00 c7 6a 01 00 a0 86 01 00 06 b8 01 00 a0 86 01 00 06 0d 02 00 a0 86 01 00
         e1 69 02 00 a0 86 01 00 b0 ce 02 00 a0 86 01 00 8d 3b 03 00 a0 86 01 00 90 b0 03 00
         a0 86 01 00 00 00 07 91",
        "30 31 30 30 03 22 01 00 00 00 00 00 64 00 00 00 11 00 00 00 64 00 00 00 23 00 00 00
         64 00 00 00 34 00 00 00 64 00 00 00 45 00 00 00 64 00 00 00 56 00 00 00 64 00 00 00
         67 00 00 00 64 00 00 00 78 00 00 00 64 00 00 00 89 00 00 00 64 00 00 00 9a 00 00 00
         64 00 00 00 aa 00 00 00 64 00 00 00 bb 00 00 00 64 00 00 00 cc 00 00 00 64 00 00 00
         dd 00 00 00 64 00 00 00 ed 00 00 00 64 00 00 00 ff 00 00 00 64 00 00 00 10 01 00 00
         64 00 00 00 00 00 00 00 64 00 00 00 08 00 00 00 64 00 00 00 11 00 00 00 64 00 00 00
         19 00 00 00 64 00 00 00 22 00 00 00 64 00 00 00 2b 00 00 00 64 00 00 00 34 00 00 00
         64 00 00 00 3d 00 00 00 64 00 00 00 46 00 00 00 64 00 00 00 50 00 00 00 64 00 00 00
         59 00 00 00 64 00 00 00 63 00 00 00 64 00 00 00 6d 00 00 00 64 00 00 00 77 00 00 00
         64 00 00 00 80 00 00 00 64 00 00 00 8a 00 00 00 64 00 00 00 93 00 00 00 64 00 00 00
         00 00 aa 73",
    ];
    /// An NRW whose raw SubIFD's NEFInfo holds `records` as its records 2 and 3.
    fn nrw(records: &[Vec<u8>; 2]) -> Vec<u8> {
        let entries: Vec<Entry> = [2u16, 3]
            .into_iter()
            .zip(records)
            .map(|(tag, r)| (tag, 7, r.len() as u32, r.clone()))
            .collect();
        let mut info = b"Nikon\0\x01\x01\0\0II*\0\x08\0\0\0".to_vec();
        info.extend(directory(8, &entries));
        tiff(&[
            vec![
                (0x14a, 4, 1, vec![0; 4]),
                (0x010f, 2, 18, b"NIKON CORPORATION\0".to_vec()),
            ],
            vec![(0xc7d5, 7, info.len() as u32, info)],
        ])
    }
    /// Against the warp Adobe DNG Converter 18.7 writes for the P1100 file, radius 1 at
    /// the 4624 × 3470 image's corner.
    #[test]
    fn reads_coolpix_tables_as_adobe_does() {
        let records = P1100.map(hex);
        let bytes = nrw(&records);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.nrw");
        std::fs::write(&p, bytes).unwrap();
        let m = frame([4624, 3470], [8, 7, 4608, 3456]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.default_on && c.vignetting.is_none());
        // Red's CA reaches past the corner, as in Adobe's warp, but only green frames
        // the photo: no zoom.
        let size = [4624., 3470.];
        assert_eq!(c.fill_scale(1., size, [8., 7., 4616., 3463.]), 1.);
        let warp = [
            [
                0.9770807182516118,
                0.02199974273005716,
                0.0017052145920701364,
                9.575715160584196e-05,
            ],
            [
                0.9763794594830819,
                0.02202587112403992,
                0.0016701011495196144,
                0.00010085923996837375,
            ],
            [
                0.9767193908434475,
                0.022103375850249615,
                0.0016335961178098911,
                0.00010094050312599621,
            ],
        ];
        let unit = 2312f64.hypot(1735.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.75, "{off} px from Adobe's warp");
        // The A1000's layout (header 3 2 17 1) is not read.
        let a1000 = P1100[0].replacen("03 03 11 01", "03 02 11 01", 1);
        let records = [hex(&a1000), records[1].clone()];
        let bytes = nrw(&records);
        std::fs::write(&p, bytes).unwrap();
        assert!(super::read(&p, &m).is_none());
    }
    /// 1-Nikon-Z7-RAW-14bit-uncompressed-L.NEF (raw.pixls.us, Z 7, NIKKOR Z 24-70mm f/4 S
    /// at 56mm f/5.6): a lens that needs the correction (mode 3).
    const Z7: [&str; 4] = [
        "30 31 30 30 03 01 00 00 9a 59 01 00 00 10 00 00 04 00 00 00 31 42 00 00 00 00 10 00
         9f d9 ff ff 00 00 10 00 de a1 00 00 00 00 10 00 00 00 00 00 01 00 00 00 00 00 00 00
         01 00 00 00 00 00 00 00 92 fd 00 00 00 00 10 00 00 00 00 00 00 00 00 00 00 00 ec 5b",
        "30 31 30 30 01 01 00 00 9a 59 01 00 00 10 00 00 08 00 00 00 6a 20 ee ff 00 00 10 00
         00 00 00 00 01 00 00 00 5e 6e 21 00 00 00 10 00 00 00 00 00 01 00 00 00 86 6b f3 ff
         00 00 10 00 00 00 00 00 01 00 00 00 80 ca 07 00 00 00 10 00 00 00 00 00 01 00 00 00
         01 00 00 00 01 00 00 00 00 00 00 00 1d 02 00 00 00 04 00 00 00 00 00 00 00 00 00 00
         00 00 d8 79",
        "30 31 30 30 01 02 00 00 9a 59 01 00 00 10 00 00 03 00 00 00 14 00 00 00 00 00 10 00
         c0 fe ff ff 00 00 10 00 00 00 00 00 01 00 00 00 99 ff ff ff 00 00 10 00 03 00 00 00
         af ff ff ff 00 00 10 00 66 01 00 00 00 00 10 00 00 00 00 00 01 00 00 00 ab 00 00 00
         00 00 10 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 2c 47",
        "30 31 30 30 36 ff ff ff 00 01 00 00 f7 01 00 00 00 01 00 00 00 00 ec e2",
    ];
    /// Nikon_Z8_high_efficiency_low.NEF (raw.pixls.us, Z 8, NIKKOR Z 14-24mm f/2.8 S at
    /// 24mm f/8): a lens whose correction is optional (mode 1).
    const Z8: [&str; 4] = [
        "30 31 30 30 01 01 00 00 9a 59 01 00 00 10 00 00 04 00 00 00 bc 24 00 00 00 00 10 00
         3a d5 ff ff 00 00 10 00 7d 54 00 00 00 00 10 00 00 00 00 00 01 00 00 00 00 00 00 00
         01 00 00 00 00 00 00 00 a7 50 00 00 00 00 10 00 00 00 00 00 00 00 00 00 00 00 fe fb",
        "30 31 30 30 01 01 00 00 9a 59 01 00 00 10 00 00 08 00 00 00 00 00 00 00 00 00 10 00
         00 00 00 00 01 00 00 00 24 26 01 00 00 00 10 00 00 00 00 00 01 00 00 00 74 33 00 00
         00 00 10 00 00 00 00 00 01 00 00 00 cf 33 08 00 00 00 10 00 00 00 00 00 01 00 00 00
         01 00 00 00 01 00 00 00 00 00 00 00 9a 01 00 00 00 04 00 00 00 00 00 00 00 00 00 00
         00 00 5e 0a",
        "30 31 30 30 01 02 00 00 9a 59 01 00 00 10 00 00 03 00 00 00 5a fd ff ff 00 00 10 00
         fa 03 00 00 00 00 10 00 00 00 00 00 01 00 00 00 2e ff ff ff 00 00 10 00 03 00 00 00
         f6 02 00 00 00 00 10 00 c1 fb ff ff 00 00 10 00 00 00 00 00 01 00 00 00 8c 01 00 00
         00 00 10 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 62 d6",
        "30 31 30 30 1c f9 ff ff 00 01 00 00 a7 04 00 00 00 01 00 00 00 00 13 e5",
    ];
    fn nikon_file(dir: &Path, records: [&str; 4]) -> std::path::PathBuf {
        let records = records.map(hex);
        let p = dir.join("a.nef");
        std::fs::write(&p, nef(records.each_ref().map(|r| &r[..]))).unwrap();
        p
    }
    /// Against the opcodes Adobe DNG Converter 18.7 writes for the Z 7 file: its
    /// WarpRectilinear and FixVignetteRadial, radius 1 at the 8288 × 5520 image's corner
    /// farthest from their centre (4143.2, 2762.0).
    #[test]
    fn reads_nikon_corrections_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([8288, 5520], [16, 8, 8256, 5504]);
        let c = super::read(&nikon_file(d.path(), Z7), &m).unwrap();
        assert!(c.default_on);
        assert_eq!(c.source, "Nikon built-in");
        // Record 8's optical centre, where Adobe's opcodes have theirs.
        let centre = c.centre_in([8288., 5520.]);
        assert!(
            (centre[0] - 4143.2).abs() < 0.05 && (centre[1] - 2762.).abs() < 0.05,
            "{centre:?}"
        );
        let warp = [
            [
                0.9417050022703349,
                0.0341640171586027,
                0.007692219047459261,
                0.0017741287665110545,
            ],
            [
                0.9417976348406594,
                0.03441966339031103,
                0.0077043443801977285,
                0.0017867041750981045,
            ],
            [
                0.9419519123251883,
                0.03469149168629527,
                0.0076797231784166645,
                0.0018015685664133994,
            ],
        ];
        let unit = 4144.8f64.hypot(2762.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.25, "{off} px from Adobe's warp");
        // Without the correction, halfway to the corner is 120 px out.
        assert!(adobe(warp[1], unit, 2481.) < 2481. - 110.);
        let k = [
            0.2572594881057739,
            -0.4153914023190737,
            1.1038962956517935,
            -0.5901794414967299,
        ];
        let half = 8288f64.hypot(5520.) / 2.;
        for px in [1000., 2500., 4000., 4962.] {
            let r2 = (px / unit).powi(2);
            let gain = 1. + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3])));
            let ours = f64::from(c.vignetting_gain((px / half) as f32));
            assert!((ours - gain).abs() < 0.003, "{px}: {ours} against {gain}");
        }
    }
    /// The Z 8's lens has an optional correction, which Adobe embeds in its DNG as the
    /// "Camera Settings" lens profile: its distortion (radius in focal lengths, 24mm on
    /// a sensor of crop factor 1.003485), piecewise vignetting and the CA of its warp.
    #[test]
    fn reads_nikon_optional_correction_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([8280, 5520], [12, 8, 8256, 5504]);
        let c = super::read(&nikon_file(d.path(), Z8), &m).unwrap();
        // Enable Profile Corrections uses it when no Adobe profile matches.
        assert!(!c.default_on);
        let profile = |px: f64| {
            let rho = px / 8256f64.hypot(5504.) * 43.2666 / 1.003485 / 24.;
            let r2 = rho * rho;
            px * 0.9808 * (1. + r2 * (0.021103 + r2 * (-0.000082 + r2 * 0.00394)))
        };
        // The warp's red and blue planes (green's is the identity), radius 1 at the
        // corner farthest from (4133.1, 2764.6).
        let plane = |k: [f64; 4], px: f64| adobe(k, 4146.9f64.hypot(2764.6), px) / px;
        let red = [
            0.9998067206878185,
            0.0007524371460466559,
            -0.0005651242131282319,
            0.00013040478604332164,
        ];
        let blue = [
            1.000369837685565,
            -0.0007921865303304411,
            0.0006318055362242725,
            -0.0001457917814567964,
        ];
        let off = largest_difference(&c, &m, &|ch, px| match ch {
            0 => profile(px) * plane(red, px),
            2 => profile(px) * plane(blue, px),
            _ => profile(px),
        });
        assert!(off < 0.45, "{off} px from Adobe's profile");
        // Piecewise vignetting: radius in focal lengths, the share of light kept.
        let half = 8280f64.hypot(5520.) / 2.;
        for (rho, kept) in [
            (0.290603, 0.979004),
            (0.581206, 0.918796),
            (0.900869, 0.807075),
        ] {
            let px = rho * 24. * 1.003485 / 43.2666 * 8256f64.hypot(5504.);
            let ours = f64::from(c.vignetting_gain((px / half) as f32));
            assert!(
                (1. / ours - kept).abs() < 0.001,
                "{rho}: {} against {kept}",
                1. / ours
            );
        }
    }
    #[test]
    fn nikon_correction_off_in_camera_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([8288, 5520], [16, 8, 8256, 5504]);
        // Mode 2: a lens that needs it, with Auto Distortion Control off (D780).
        let off = Z7[0].replacen("03 01 00 00", "02 01 00 00", 1);
        let p = nikon_file(d.path(), [&off, Z7[1], Z7[2], Z7[3]]);
        assert!(super::read(&p, &m).is_none());
    }
    /// An SRW with IFD0's Make and Model and a maker note of `note` entries (tag,
    /// type, count, data in the file's byte order); offsets in the note count from
    /// its start.
    fn srw(little: bool, model: &str, note: &[Entry]) -> Vec<u8> {
        let int = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let short = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let directory = |at: usize, entries: &[Entry]| {
            let mut data_at = at + 2 + entries.len() * 12 + 4;
            let (mut out, mut data): (Vec<u8>, Vec<u8>) =
                (short(entries.len() as u16).to_vec(), vec![]);
            for (tag, kind, count, bytes) in entries {
                out.extend(short(*tag));
                out.extend(short(*kind));
                out.extend(int(*count));
                if bytes.len() <= 4 {
                    out.extend(bytes.iter().chain(&[0; 4]).take(4));
                } else {
                    out.extend(int(data_at as u32));
                    data.extend(bytes);
                    data_at += bytes.len();
                }
            }
            out.extend(int(0));
            out.extend(data);
            out
        };
        let note = directory(0, note);
        let make = b"SAMSUNG\0".to_vec();
        let model = [model.as_bytes(), b"\0"].concat();
        let ifd0 = |exif: u32| {
            directory(
                8,
                &[
                    (0x010f, 2, make.len() as u32, make.clone()),
                    (0x0110, 2, model.len() as u32, model.clone()),
                    (0x8769, 4, 1, int(exif).to_vec()),
                ],
            )
        };
        let exif_at = 8 + ifd0(0).len();
        let exif =
            |at: u32| directory(exif_at, &[(0x927c, 7, note.len() as u32, int(at).to_vec())]);
        let note_at = exif_at + exif(0).len();
        let mut out = if little { b"II*\0" } else { b"MM\0*" }.to_vec();
        out.extend(int(8));
        out.extend(ifd0(exif_at as u32));
        out.extend(exif(note_at as u32));
        out.extend(note);
        // At least as long as the header `read` reads.
        out.resize(out.len().max(128), 0);
        out
    }
    /// A Samsung maker note's LensType (0xA003), EncryptionKey (0xA020), Distortion
    /// (0xA050) and ChromaticAberration (0xA051), as stored.
    struct SamsungTags<'a> {
        little: bool,
        model: &'a str,
        lens: &'a str,
        key: &'a str,
        distortion: &'a str,
        ca: &'a str,
    }
    const BIG_KEY: &str = "00 00 01 31 00 00 00 48 00 00 02 e1 00 00 01 c8 00 00 01 1a 00 00 01 33
        00 00 02 07 00 00 02 d4 00 00 00 0d 00 00 01 f9 00 00 00 c1";
    /// 2013-05-08-194524__sam6589.srw (raw.pixls.us, NX200, NX 16mm F2.4).
    const NX200: SamsungTags<'static> = SamsungTags {
        little: false,
        model: "NX200",
        lens: "00 08",
        key: BIG_KEY,
        distortion: "ff ff fe d0 ff ff ff b8 ff ff db 9f ff ff fe 2c 00 00 05 6e ff ff fe ba
            ff ff fd f9 ff ff fd 2c",
        ca: "00 07 03 c3 05 6c 0a 97 0a c7 0b 62 0c c4 0e e1 29 ee 27 27 29 13 27 db 28 4b
            27 62 29 fb 28 d7 28 29 28 42 29 16 29 e3 27 1c 27 1c",
    };
    /// RAW_SAMSUNG_EX2F.SRW (raw.pixls.us, EX2F).
    const EX2F: SamsungTags<'static> = SamsungTags {
        little: false,
        model: "EX2F",
        lens: "00 00",
        key: BIG_KEY,
        distortion: "ff ff fe d0 ff ff ff b8 ff ff f8 8d ff ff fe 2d ff ff ff ae ff ff fe ba
            ff ff fd f9 ff ff fd 2c",
        ca: "00 07 02 77 02 d3 06 b2 06 df 07 77 08 d5 0a ef 29 e1 27 1a 29 06 27 ce 28 3e
            27 55 29 ee 28 f2 28 44 28 5d 29 31 29 fe 27 37 27 37",
    };
    /// SAM_0295.SRW (raw.pixls.us, NX mini, NX-M 9mm F3.5): a little-endian file.
    const NX_MINI: SamsungTags<'static> = SamsungTags {
        little: true,
        model: "NX mini",
        lens: "00 00 00 00",
        key: "31 01 00 00 48 00 00 00 e1 02 00 00 c8 01 00 00 1a 01 00 00 33 01 00 00
            07 02 00 00 d4 02 00 00 0d 00 00 00 f9 01 00 00 c1 00 00 00",
        distortion: "d0 fe ff ff b8 ff ff ff 46 dc ff ff 2c fe ff ff e4 02 00 00 ba fe ff ff
            f9 fd ff ff 2c fd ff ff",
        ca: "07 00 c3 03 6c 05 97 0a c7 0a 62 0b c4 0c e1 0e ea 29 23 27 0f 29 d7 27 47 28
            5e 27 f7 29 d8 28 2a 28 43 28 17 29 e4 29 1d 27 1d 27",
    };
    fn samsung_file(dir: &Path, s: &SamsungTags) -> std::path::PathBuf {
        let lens = hex(s.lens);
        let note = [
            (0xa003, 3, lens.len() as u32 / 2, lens),
            (0xa020, 4, 11, hex(s.key)),
            (0xa050, 9, 8, hex(s.distortion)),
            (0xa051, 3, 22, hex(s.ca)),
        ];
        let p = dir.join("a.srw");
        std::fs::write(&p, srw(s.little, s.model, &note)).unwrap();
        p
    }
    /// Against the warp Adobe DNG Converter 18.7 writes for the NX200 file: radius 1
    /// at the corner of its 5528 × 3670 ActiveArea farthest from the default crop's
    /// centre, (2771, 1839).
    #[test]
    fn reads_samsung_corrections_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([5530, 3694], [28, 22, 5472, 3648]);
        let c = super::read(&samsung_file(d.path(), &NX200), &m).unwrap();
        assert!(c.default_on && c.vignetting.is_none());
        assert_eq!(c.source, "Samsung built-in");
        // The default crop's centre, where Adobe's warp has its centre.
        assert_eq!(c.centre_in([5530., 3694.]), [2764., 1846.]);
        let warp = [
            [
                1.0005000000000004,
                -0.09490109134425251,
                0.020464071708261592,
                0.,
            ],
            [1., -0.094853664512, 0.02045384478587856, 0.],
            [
                0.9999499999999998,
                -0.09484892182877282,
                0.02045282209363186,
                0.,
            ],
        ];
        let unit = 2771f64.hypot(1839.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
        let (ca, most) = largest_ca_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(ca < 0.02, "{ca} px from Adobe's CA");
        assert!(most > 1., "Adobe corrects {most} px");
        // Without the correction the corner is 250 px out.
        assert!(adobe(warp[1], unit, 3300.) < 3300. - 240.);
    }
    /// The EX2F's red and blue scale by 0.75 and 0.25 of their stored change where the
    /// interchangeable-lens bodies' scale by half (radius 1 at the corner farthest from
    /// (2044, 1534) in a 4070 × 3046 ActiveArea).
    #[test]
    fn reads_samsung_compact_chromatic_aberration_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([4040, 3029], [20, 14, 4000, 3000]);
        let c = super::read(&samsung_file(d.path(), &EX2F), &m).unwrap();
        let warp = [
            [0.999775, -0.07639658330031347, 0.0008529113060220778, 0.],
            [1., -0.0764137764, 0.00085310325424928, 0.],
            [1.00065, -0.07646344535466416, 0.000853657771376213, 0.],
        ];
        let unit = 2044f64.hypot(1534.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
    }
    #[test]
    fn reads_little_endian_samsung_notes() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([5544, 3692], [36, 22, 5472, 3648]);
        let c = super::read(&samsung_file(d.path(), &NX_MINI), &m).unwrap();
        let warp = [
            [1.0003, -0.09187401553847731, 0.01219599728573692, 0.],
            [1., -0.09184646159999998, 0.012192339583871994, 0.],
            [1., -0.09184646159999642, 0.012192339583864665, 0.],
        ];
        let unit = 2748f64.hypot(1836.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
    }
    /// Adobe ignores the camera's data for lenses and compacts it has its own profiles
    /// for, and on the first NX bodies.
    #[test]
    fn samsung_data_adobe_does_not_use_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([5530, 3694], [28, 22, 5472, 3648]);
        for lens in ["00 01", "00 02", "00 03"] {
            let p = samsung_file(d.path(), &SamsungTags { lens, ..NX200 });
            assert!(super::read(&p, &m).is_none(), "{lens}");
        }
        for model in ["NX5", "NX10", "NX100", "NX11", "EX1", "WB2000"] {
            let p = samsung_file(d.path(), &SamsungTags { model, ..NX200 });
            assert!(super::read(&p, &m).is_none(), "{model}");
        }
    }
    /// A distortion model that is zero at the centre is not one; Adobe keeps the CA.
    #[test]
    fn samsung_chromatic_aberration_without_distortion() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([5530, 3694], [28, 22, 5472, 3648]);
        // The constant's mantissa 1 stored as 0.
        let distortion = NX200.distortion.replacen("ff ff fe d0", "ff ff fe cf", 1);
        let tags = SamsungTags {
            distortion: &distortion,
            ..NX200
        };
        let c = super::read(&samsung_file(d.path(), &tags), &m).unwrap();
        assert!(c.distortion.is_none());
        let [red, blue] = c.chromatic.as_ref().unwrap();
        assert!((red.eval(0.5) - 1.0005).abs() < 1e-6);
        assert!((blue.eval(0.9) - 0.99995).abs() < 1e-6);
    }
    /// An ISO base media box.
    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        [&(8 + body.len() as u32).to_be_bytes()[..], kind, body].concat()
    }
    /// A CR3 of `model` whose timed metadata (the CTMD track's one sample) is `records`.
    fn cr3(model: &str, records: &[u8]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        let name = [model.as_bytes(), b"\0"].concat();
        let cmt1 = tiff(&[vec![(0x0110, 2, name.len() as u32, name)]]);
        let canon = [&super::CANON_UUID[..], &bx(b"CMT1", &cmt1)].concat();
        let moov = |at: u64| {
            let entry = bx(b"CTMD", &[0, 0, 0, 0, 0, 0, 0, 1]);
            let stsd = [&[0, 0, 0, 0, 0, 0, 0, 1][..], &entry].concat();
            let stsz = [
                [0; 4],
                (records.len() as u32).to_be_bytes(),
                1u32.to_be_bytes(),
            ]
            .concat();
            let co64 = [&[0, 0, 0, 0, 0, 0, 0, 1][..], &at.to_be_bytes()].concat();
            let stbl = [bx(b"stsd", &stsd), bx(b"stsz", &stsz), bx(b"co64", &co64)].concat();
            let trak = bx(b"trak", &bx(b"mdia", &bx(b"minf", &bx(b"stbl", &stbl))));
            bx(b"moov", &[bx(b"uuid", &canon), trak].concat())
        };
        let at = (ftyp.len() + moov(0).len() + 8) as u64;
        [ftyp, moov(at), bx(b"mdat", records)].concat()
    }
    /// The timed metadata's records: a time stamp, then record 14 holding `tables`.
    fn ctmd(tables: &[u16]) -> Vec<u8> {
        let record = |kind: u16, data: &[u8]| {
            let len = (12 + data.len()) as u32;
            [
                &len.to_le_bytes()[..],
                &kind.to_le_bytes(),
                &[0, 0, 0, 1, 0xff, 0xff],
                data,
            ]
            .concat()
        };
        let lens: Vec<u8> = tables.iter().flat_map(|v| v.to_le_bytes()).collect();
        [record(1, &[0; 12]), record(14, &lens)].concat()
    }
    /// Record 14 of IMG_4241.CR3 (raw.pixls.us, EOS R6 Mark III, RF50mm F1.4 L VCM at
    /// f/7.1): the counts, then seven arrays of 23 slots: vignetting and its knots, red,
    /// blue and their knots, distortion and its knots.
    const R6III: [u16; 164] = [
        17, 17, 17, 8191, 8079, 7767, 7550, 7302, 7028, 6734, 6428, 6275, 6124, 5975, 5889, 5834,
        5766, 5692, 5505, 5095, 0, 0, 0, 0, 0, 0, 0, 757, 1514, 1893, 2271, 2650, 3028, 3407, 3596,
        3786, 3975, 4088, 4164, 4259, 4353, 4448, 4543, 0, 0, 0, 0, 0, 0, 16384, 16398, 16409,
        16416, 16426, 16441, 16465, 16502, 16531, 16566, 16612, 16646, 16671, 16704, 16738, 16773,
        16807, 0, 0, 0, 0, 0, 0, 16384, 16442, 16499, 16520, 16531, 16526, 16499, 16436, 16384,
        16316, 16226, 16158, 16108, 16038, 15959, 15881, 15803, 0, 0, 0, 0, 0, 0, 0, 757, 1514,
        1893, 2271, 2650, 3028, 3407, 3596, 3786, 3975, 4088, 4164, 4259, 4353, 4448, 4543, 0, 0,
        0, 0, 0, 0, 16384, 16379, 16412, 16437, 16466, 16499, 16532, 16562, 16576, 16586, 16594,
        16596, 16596, 16595, 16593, 16588, 16582, 0, 0, 0, 0, 0, 0, 0, 757, 1514, 1893, 2271, 2650,
        3028, 3407, 3596, 3786, 3975, 4088, 4164, 4259, 4353, 4448, 4543, 0, 0, 0, 0, 0, 0,
    ];
    /// Record 14 of RAW.CR3 (raw.pixls.us, EOS R5 Mark II, RF135mm F1.8 L IS USM at f/1.8).
    const R5II: [u16; 164] = [
        17, 17, 17, 8191, 8138, 7334, 6790, 6224, 5644, 5057, 4477, 4191, 3910, 3632, 3446, 3307,
        3109, 2796, 2388, 1978, 0, 0, 0, 0, 0, 0, 0, 757, 1514, 1893, 2271, 2650, 3028, 3407, 3596,
        3786, 3975, 4088, 4164, 4259, 4353, 4448, 4543, 0, 0, 0, 0, 0, 0, 16384, 16277, 16174,
        16121, 16065, 16010, 15957, 15911, 15894, 15884, 15885, 15894, 15906, 15916, 15927, 15939,
        15950, 0, 0, 0, 0, 0, 0, 16384, 16487, 16574, 16615, 16660, 16707, 16754, 16795, 16811,
        16821, 16814, 16798, 16778, 16759, 16739, 16720, 16700, 0, 0, 0, 0, 0, 0, 0, 757, 1514,
        1893, 2271, 2650, 3028, 3407, 3596, 3786, 3975, 4088, 4164, 4259, 4353, 4448, 4543, 0, 0,
        0, 0, 0, 0, 16384, 16344, 16409, 16456, 16511, 16573, 16639, 16703, 16732, 16759, 16780,
        16789, 16795, 16798, 16800, 16799, 16797, 0, 0, 0, 0, 0, 0, 0, 757, 1514, 1893, 2271, 2650,
        3028, 3407, 3596, 3786, 3975, 4088, 4164, 4259, 4353, 4448, 4543, 0, 0, 0, 0, 0, 0,
    ];
    fn canon_file(dir: &Path, model: &str, tables: &[u16]) -> std::path::PathBuf {
        let p = dir.join("a.cr3");
        std::fs::write(&p, cr3(model, &ctmd(tables))).unwrap();
        p
    }
    /// What Adobe DNG Converter 18.7 writes into a DNG of a Canon file: the "Camera
    /// Settings" profile (focal length, sensor format factor, ScaleFactor, radial
    /// distortion, piecewise vignetting) and the red and blue planes of its warp
    /// (green's is the identity), whose radius 1 is the farthest corner of the DNG's
    /// active area from its centre, which is the default crop's.
    struct CanonAdobe {
        focal: f64,
        format_factor: f64,
        scale: f64,
        k: [f64; 3],
        piecewise: &'static [(f64, f64)],
        warp: [[f64; 4]; 2],
        corner: [f64; 2],
    }
    /// The profile's source radius in pixels at output radius `px`, for a default crop
    /// of `crop` pixels: its radius is in focal lengths, the crop's diagonal being
    /// 43.2666 mm over the sensor format factor.
    fn canon_profile(a: &CanonAdobe, crop: [u32; 2], px: f64) -> f64 {
        let diagonal = f64::from(crop[0]).hypot(f64::from(crop[1]));
        let rho = px / diagonal * 43.2666 / a.format_factor / a.focal;
        let r2 = rho * rho;
        px * a.scale * (1. + r2 * (a.k[0] + r2 * (a.k[1] + r2 * a.k[2])))
    }
    fn check_canon(c: &LensCorrection, m: &Metadata, a: &CanonAdobe, within: f64) {
        assert!(!c.default_on);
        assert_eq!(c.source, "Canon built-in");
        let crop = [m.crop_width, m.crop_height];
        let unit = a.corner[0].hypot(a.corner[1]);
        let plane = |k: [f64; 4], px: f64| adobe(k, unit, px) / px;
        let off = largest_difference(c, m, &|ch, px| {
            let green = canon_profile(a, crop, px);
            match ch {
                0 => green * plane(a.warp[0], green),
                2 => green * plane(a.warp[1], green),
                _ => green,
            }
        });
        assert!(off < within, "{off} px from Adobe's profile");
        let half = f64::from(m.width).hypot(f64::from(m.height)) / 2.;
        let diagonal = f64::from(crop[0]).hypot(f64::from(crop[1]));
        for &(rho, kept) in a.piecewise {
            let px = rho * a.focal * a.format_factor / 43.2666 * diagonal;
            let ours = 1. / f64::from(c.vignetting_gain((px / half) as f32));
            assert!((ours - kept).abs() < 0.001, "{rho}: {ours} against {kept}");
        }
    }
    #[test]
    fn reads_canon_r6_mark_iii_corrections_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let p = canon_file(d.path(), "Canon EOS R6 Mark III", &R6III);
        let m = frame([6959, 4639], [0, 0, 6960, 4640]);
        let c = super::read(&p, &m).unwrap();
        let adobe = CanonAdobe {
            focal: 50.,
            format_factor: 1.,
            scale: 0.989001,
            k: [0.083019, 0.016376, -0.764324],
            piecewise: &[
                (0.080435, 0.986326),
                (0.16087, 0.948236),
                (0.241305, 0.891466),
                (0.32174, 0.822122),
                (0.402282, 0.74765),
                (0.442446, 0.712245),
                (0.482717, 0.622024),
            ],
            warp: [
                [
                    1.0000037075930996,
                    -4.231126716690595e-06,
                    2.083027506703311e-05,
                    1.2193982455706968e-05,
                ],
                [
                    1.0000062828295775,
                    6.320382627848142e-05,
                    -7.571489431295625e-05,
                    -2.270913473936408e-05,
                ],
            ],
            corner: [3492., 2332.],
        };
        check_canon(&c, &m, &adobe, 0.15);
        // Without the correction, halfway to the corner is 15 px out.
        assert!(canon_profile(&adobe, [6960, 4640], 2091.) < 2091. - 14.);
    }
    #[test]
    fn reads_canon_r5_mark_ii_corrections_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let p = canon_file(d.path(), "Canon EOS R5m2", &R5II);
        let m = frame([8222, 5488], [14, 14, 8192, 5464]);
        let c = super::read(&p, &m).unwrap();
        let adobe = CanonAdobe {
            focal: 135.,
            format_factor: 1.001926,
            scale: 0.97803,
            k: [1.135087, 0.747337, -422.990386],
            piecewise: &[
                (0.029741, 0.993529),
                (0.059481, 0.895373),
                (0.089222, 0.759858),
                (0.118962, 0.617385),
                (0.148742, 0.477353),
                (0.163593, 0.403736),
                (0.178483, 0.241485),
            ],
            warp: [
                [
                    0.9999888454181883,
                    -0.00010766927304507836,
                    5.443760114059718e-05,
                    4.93985093601397e-06,
                ],
                [
                    1.0000122089263515,
                    7.45386255399043e-05,
                    -5.82086411194138e-06,
                    -3.114694366284039e-05,
                ],
            ],
            corner: [4112., 2746.],
        };
        // 0.16 px at the corner, where Adobe's ScaleFactor frames 2·10⁻⁵ tighter.
        check_canon(&c, &m, &adobe, 0.2);
    }
    /// Adobe DNG Converter's profiles of copies of IMG_4241.CR3 with edited distortion
    /// tables: values above 1.0275 count as 1.0275, and a table's change from 1 is
    /// fitted at 80% of its size.
    #[test]
    fn fits_canon_distortion_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([6959, 4639], [0, 0, 6960, 4640]);
        // The distortion's values and knots are the sixth and seventh arrays.
        let (values, knots) = (3 + 5 * 23, 3 + 6 * 23);
        let knots: [u16; 17] = R6III[knots..knots + 17].try_into().unwrap();
        for (name, table, k) in [
            (
                "a quartic",
                knots.map(|x| (16384. * (1. + 0.01 * (f64::from(x) / 4543.).powi(4))).round()),
                [-0.000256, 0.147874, 0.000152],
            ),
            (
                "a spike past the limit",
                std::array::from_fn(|i| if i == 6 { 18024. } else { 16384. }),
                [0.531223, -4.283277, 9.71879],
            ),
        ] {
            let mut tables = R6III;
            for (slot, v) in tables[values..values + 17].iter_mut().zip(table) {
                *slot = v as u16;
            }
            let c =
                super::read(&canon_file(d.path(), "Canon EOS R6 Mark III", &tables), &m).unwrap();
            let scale = |px: f64| {
                let rho = px / 6960f64.hypot(4640.) * 43.2666 / 50.;
                let r2 = rho * rho;
                1. + r2 * (k[0] + r2 * (k[1] + r2 * k[2]))
            };
            // Both framed to keep the default crop's corner.
            let corner = 6960f64.hypot(4640.) / 2.;
            let half = 6959f64.hypot(4639.) / 2.;
            let ours = |px: f64| f64::from(c.radial_scale_with((px / half) as f32, 1.)[1]);
            let off = (0..=100)
                .map(|i| corner * f64::from(i) / 100.)
                .map(|px| px * (ours(px) / ours(corner) - scale(px) / scale(corner)).abs())
                .fold(0., f64::max);
            assert!(off < 0.3, "{name}: {off} px from Adobe's profile");
        }
    }
    /// Adobe applies no camera data for other bodies, although the R50 V stores the
    /// same record.
    #[test]
    fn canon_tables_of_other_bodies_are_not_read() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([6959, 4639], [0, 0, 6960, 4640]);
        let p = canon_file(d.path(), "Canon EOS R50 V", &R6III);
        assert!(super::read(&p, &m).is_none());
    }
    #[test]
    fn malformed_canon_tables_are_not_read() {
        let d = tempfile::tempdir().unwrap();
        let m = frame([6959, 4639], [0, 0, 6960, 4640]);
        let with = |i: usize, v: u16| {
            let mut t = R6III;
            t[i] = v;
            t
        };
        for (name, t) in [
            ("count past the slots", with(0, 24)),
            ("single knot", with(2, 1)),
            ("knots out of order", with(3 + 23 + 5, 9000)),
        ] {
            let c = super::read(&canon_file(d.path(), "Canon EOS R6 Mark III", &t), &m);
            let c = c.unwrap_or_default();
            let part = match name {
                "single knot" => c.distortion.is_none(),
                _ => c.vignetting.is_none(),
            };
            assert!(part, "{name}");
        }
        // Counts past the slots, of each group: the other groups are still read.
        for (i, count) in [0, 1, 2].into_iter().flat_map(|i| [(i, 24), (i, u16::MAX)]) {
            let p = canon_file(d.path(), "Canon EOS R6 Mark III", &with(i, count));
            let c = super::read(&p, &m).unwrap();
            let parts = [
                c.vignetting.is_none(),
                c.chromatic.is_none(),
                c.distortion.is_none(),
            ];
            assert_eq!(
                parts,
                std::array::from_fn(|j| j == i),
                "count {i} = {count}"
            );
        }
        let short = &R6III[..100];
        assert!(super::read(&canon_file(d.path(), "Canon EOS R6 Mark III", short), &m).is_none());
    }
    #[test]
    fn unsupported_or_truncated_files_have_no_correction() {
        let d = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("empty", &b""[..]),
            ("text", &[b'x'; 200][..]),
            ("tiff", b"II*\0\x08\0\0\0\0\0"),
        ] {
            let p = d.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            assert!(read(&p).is_none(), "{name}");
        }
    }
}
