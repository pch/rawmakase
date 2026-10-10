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
//!   units; vignetting v restores 2^(2^(v/8192 - 1) - 0.5).
//!
//! The Fujifilm vignetting agrees with the FixVignetteRadial opcode Adobe writes into
//! DNGs of the same file to within 1% (see docs/lens-corrections.md).
use crate::optics::{LensCorrection, Radial};
use crate::tiff::Tiff;
use std::{fs::File, io::Read, path::Path};

/// Returns `None` for unsupported files or when the camera stored no corrections.
pub fn read(path: &Path) -> Option<LensCorrection> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 108];
    f.read_exact(&mut head).ok()?;
    let c = if head.starts_with(b"FUJIFILMCCD-RAW") {
        let base = u32::from_be_bytes(head[100..104].try_into().ok()?) as u64;
        fuji(&mut Tiff::open(f, base)?)
    } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        sony(&mut Tiff::open(f, 0)?)
    } else {
        None
    }?;
    (!c.is_empty() && c.validate()).then_some(c)
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
    let distortion = distortion
        .and_then(|v| pairs(&v, &|p| 1. + p / 100.))
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
    })
}

fn sony(t: &mut Tiff) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
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
    let distortion = single(distortion, &|d| 1. + d / 16384.)
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-6));
    let chromatic = counted(chromatic, 2).and_then(|v| {
        let n = v.len() / 2;
        let scale = |s: &[f32]| s.iter().map(|x| 1. + x / 2_097_152.).collect();
        Some([
            Radial::new(knots(n), scale(&v[..n]))?,
            Radial::new(knots(n), scale(&v[n..]))?,
        ])
    });
    Some(LensCorrection {
        source: "Sony built-in".into(),
        // Not yet compared with Lightroom, which may apply its own profile instead.
        default_on: false,
        vignetting,
        distortion,
        chromatic,
    })
}

#[cfg(test)]
mod tests {
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
        let c = super::read(&p).unwrap();
        assert!(!c.default_on);
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
        let entry = |tag, v: &[i16]| (tag, 8, v.len() as u32, shorts(v));
        let bytes = tiff(&[
            vec![(0x14a, 4, 1, vec![0; 4])],
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
        let c = super::read(&sony_file(d.path(), &ZV1_VIG, &ZV1_CA, &ZV1_DIST)).unwrap();
        let dist = c.distortion.as_ref().unwrap();
        assert_eq!(dist.knots.len(), 11);
        assert_eq!(dist.knots[1], 0.1);
        assert!((dist.eval(0.) - (1. + 1265. / 16384.)).abs() < 1e-6);
        assert!((dist.eval(1.) - (1. - 1725. / 16384.)).abs() < 1e-6);
        let [red, blue] = c.chromatic.as_ref().unwrap();
        assert_eq!(red.knots.len(), 11);
        assert!((red.eval(1.) - (1. + 1408. / 2_097_152.)).abs() < 1e-7);
        assert!((blue.eval(0.1) - (1. + 1280. / 2_097_152.)).abs() < 1e-7);
        assert!((blue.eval(1.) - (1. + 256. / 2_097_152.)).abs() < 1e-7);
        assert_eq!(c.vignetting.as_ref().unwrap().knots.len(), 16);
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
            let c = super::read(&sony_file(d.path(), &ZV1_VIG, &ca, &dist)).unwrap();
            assert!(c.distortion.is_none() && c.chromatic.is_none(), "{name}");
        }
        let odd = with(ZV1_CA, 0, 21);
        let c = super::read(&sony_file(d.path(), &ZV1_VIG, &odd, &ZV1_DIST)).unwrap();
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
        let c = super::read(&p).unwrap();
        assert!(c.default_on);
        assert!(c.distortion.is_none(), "identity distortion is omitted");
        let v = c.vignetting.as_ref().unwrap();
        let gain = |p: f32| (100. / p).powf(super::FUJI_VIGNETTE_STRENGTH);
        assert!((v.eval(1.) - gain(60.4)).abs() < 1e-4);
        assert!((v.eval(0.85) - (gain(75.8) + gain(68.3)) / 2.).abs() < 1e-4);
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
            assert!(super::read(&p).is_none(), "{name}");
        }
    }
}
