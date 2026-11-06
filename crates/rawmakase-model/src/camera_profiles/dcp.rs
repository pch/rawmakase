use super::{CameraProfile, DynamicRange, Matrix, Table};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
struct Tag {
    kind: u16,
    count: usize,
    bytes: Vec<u8>,
}
struct Dcp {
    tags: BTreeMap<u16, Tag>,
    little: bool,
}
impl Dcp {
    fn read(b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= 8 && b.len() <= 16_000_000, "Invalid DCP size");
        let little = &b[..2] == b"II";
        ensure!(little || &b[..2] == b"MM", "Invalid DCP byte order");
        let u16at = |o: usize| -> Result<u16> {
            let a = b.get(o..o + 2).context("Truncated DCP")?.try_into()?;
            Ok(if little {
                u16::from_le_bytes(a)
            } else {
                u16::from_be_bytes(a)
            })
        };
        let u32at = |o: usize| -> Result<u32> {
            let a = b.get(o..o + 4).context("Truncated DCP")?.try_into()?;
            Ok(if little {
                u32::from_le_bytes(a)
            } else {
                u32::from_be_bytes(a)
            })
        };
        ensure!([42, 0x4352].contains(&u16at(2)?), "Invalid DCP header");
        let off = u32at(4)? as usize;
        let n = u16at(off)? as usize;
        ensure!(n < 512, "Too many profile tags");
        let mut tags = BTreeMap::new();
        for i in 0..n {
            let p = off + 2 + i * 12;
            let id = u16at(p)?;
            let kind = u16at(p + 2)?;
            let count = u32at(p + 4)? as usize;
            let size = match kind {
                1 | 2 | 7 => 1,
                3 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => anyhow::bail!("Unsupported DCP tag type {kind}"),
            };
            let len = count.checked_mul(size).context("DCP size overflow")?;
            let start = if len <= 4 {
                p + 8
            } else {
                u32at(p + 8)? as usize
            };
            let bytes = b
                .get(start..start.checked_add(len).context("DCP offset overflow")?)
                .context("DCP tag outside file")?
                .to_vec();
            ensure!(
                tags.insert(id, Tag { kind, count, bytes }).is_none(),
                "Duplicate DCP tag"
            );
        }
        Ok(Self { tags, little })
    }
    fn numbers(&self, id: u16) -> Result<Vec<f32>> {
        let t = self
            .tags
            .get(&id)
            .context(format!("Missing DCP tag {id}"))?;
        let u32at = |o| {
            let a = t.bytes[o..o + 4].try_into().unwrap();
            if self.little {
                u32::from_le_bytes(a)
            } else {
                u32::from_be_bytes(a)
            }
        };
        let v = (0..t.count)
            .map(|i| match t.kind {
                3 => {
                    let a = t.bytes[i * 2..i * 2 + 2].try_into().unwrap();
                    (if self.little {
                        u16::from_le_bytes(a)
                    } else {
                        u16::from_be_bytes(a)
                    }) as f32
                }
                4 => u32at(i * 4) as f32,
                11 => f32::from_bits(u32at(i * 4)),
                10 => (u32at(i * 8) as i32 as f32) / (u32at(i * 8 + 4) as i32 as f32),
                5 => u32at(i * 8) as f32 / u32at(i * 8 + 4) as f32,
                _ => f32::NAN,
            })
            .collect::<Vec<_>>();
        ensure!(
            v.iter().all(|v| v.is_finite()),
            "Invalid numeric DCP tag {id}"
        );
        Ok(v)
    }
    fn scalar(&self, id: u16, default: f32) -> Result<f32> {
        if !self.tags.contains_key(&id) {
            return Ok(default);
        }
        let v = self.numbers(id)?;
        ensure!(v.len() == 1, "Invalid scalar tag");
        Ok(v[0])
    }
    fn text(&self, id: u16) -> String {
        self.tags
            .get(&id)
            .map(|t| {
                String::from_utf8_lossy(&t.bytes)
                    .trim_end_matches('\0')
                    .to_string()
            })
            .unwrap_or_default()
    }
    /// The colour temperature of a CalibrationIlluminant tag, D65 when absent.
    fn kelvin(&self, id: u16) -> Result<f32> {
        let v = self.scalar(id, 21.)?;
        Ok(match v as u32 {
            17 => 2856.,
            21 => 6504.,
            23 => 5003.,
            20 => 5503.,
            22 => 7504.,
            // Daylight, flash (as the DNG SDK takes it; Phase One's profiles
            // use it) and fine weather.
            1 | 4 | 9 => 5500.,
            24 => 3200.,
            _ => anyhow::bail!("Unsupported calibration illuminant {v}"),
        })
    }
    /// The second illuminant, which defaults to the first.
    fn kelvin2(&self) -> Result<f32> {
        if self.tags.contains_key(&50779) {
            self.kelvin(50779)
        } else {
            self.kelvin(50778)
        }
    }
    fn matrix(&self, id: u16) -> Result<Matrix> {
        let a = self.numbers(id)?;
        ensure!(a.len() == 9, "Profile must have three color channels");
        Ok(std::array::from_fn(|i| {
            std::array::from_fn(|j| a[i * 3 + j])
        }))
    }
    /// `ProfileDynamicRange` (tag 52551): eight bytes in the file's byte order, a
    /// version (1), the profile's dynamic range (0 standard, 1 high) and a float hint
    /// of the brightest output the profile is made for, where 0 gives none. Without
    /// the tag a profile is standard dynamic range.
    fn dynamic_range(&self) -> Result<DynamicRange> {
        let Some(t) = self.tags.get(&52551) else {
            return Ok(DynamicRange::Standard);
        };
        ensure!(
            t.kind == 7 && t.count == 8,
            "Invalid ProfileDynamicRange tag"
        );
        let u16at = |o: usize| {
            let a = [t.bytes[o], t.bytes[o + 1]];
            if self.little {
                u16::from_le_bytes(a)
            } else {
                u16::from_be_bytes(a)
            }
        };
        let version = u16at(0);
        ensure!(
            version == 1,
            "Unsupported ProfileDynamicRange version {version}"
        );
        let hint: [u8; 4] = t.bytes[4..8].try_into()?;
        let hint = if self.little {
            f32::from_le_bytes(hint)
        } else {
            f32::from_be_bytes(hint)
        };
        ensure!(
            hint.is_finite() && hint >= 0.,
            "Invalid ProfileDynamicRange hint"
        );
        match u16at(2) {
            0 => Ok(DynamicRange::Standard),
            1 => Ok(DynamicRange::High),
            other => anyhow::bail!("Unsupported profile dynamic range {other}"),
        }
    }
    fn table(&self, dims: u16, data: u16, encoding: u16) -> Result<Option<Table>> {
        if !self.tags.contains_key(&data) {
            return Ok(None);
        }
        let d = self.numbers(dims)?;
        ensure!(d.len() == 3, "Invalid table dimensions");
        let a = self.numbers(data)?;
        ensure!(a.len() % 3 == 0, "Invalid table data");
        let e = self.scalar(encoding, 0.)?;
        ensure!(e == 0. || e == 1., "Unsupported table encoding");
        let t = Table {
            dims: [d[0] as usize, d[1] as usize, d[2] as usize],
            data: a
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| [p[0], p[1], p[2]])
                .collect(),
            srgb: e == 1.,
        };
        t.validate()?;
        Ok(Some(t))
    }
}
pub fn from_bytes(b: &[u8]) -> Result<CameraProfile> {
    let p = parse(b)?;
    p.validate()?;
    Ok(p)
}

/// [`from_bytes`] without the profile's validation, which also refuses what this
/// release cannot render yet (HDR profiles).
pub(crate) fn parse(b: &[u8]) -> Result<CameraProfile> {
    let d = Dcp::read(b)?;
    ensure!(
        !d.tags.contains_key(&52531)
            && !d.tags.contains_key(&52537)
            && !d.tags.contains_key(&52538),
        "Triple-illuminant profiles are not supported"
    );
    ensure!(
        d.scalar(50941, 0.)? != 2.,
        "This profile prohibits embedding; recipes embed profiles for reproducibility"
    );
    let f1 = d
        .matrix(50964)
        .context("A forward-matrix DCP is required; matrix-only profiles are not yet supported")?;
    let f2 = if d.tags.contains_key(&50965) {
        d.matrix(50965)?
    } else {
        f1
    };
    let tone = if d.tags.contains_key(&50940) {
        let a = d.numbers(50940)?;
        ensure!(a.len() % 2 == 0, "Invalid tone curve");
        a.as_chunks::<2>().0.iter().map(|p| [p[0], p[1]]).collect()
    } else {
        crate::camera_profiles::dng_tone::DEFAULT_TONE
            .iter()
            .enumerate()
            .map(|(i, y)| [i as f32 / 1024., *y])
            .collect()
    };
    let p = CameraProfile {
        enhanced: None,
        name: d.text(50936),
        camera: d.text(50708),
        copyright: d.text(50942),
        color1: d
            .tags
            .contains_key(&50721)
            .then(|| d.matrix(50721))
            .transpose()?,
        color2: d
            .tags
            .contains_key(&50722)
            .then(|| d.matrix(50722))
            .transpose()?,
        calibration_signature: d.text(50932),
        matrix_calibration_signature: None,
        forward1: f1,
        forward2: f2,
        kelvin1: d.kelvin(50778)?,
        kelvin2: d.kelvin2()?,
        hue1: d.table(50937, 50938, 51107)?,
        hue2: d.table(50937, 50939, 51107)?,
        look: d.table(50981, 50982, 51108)?,
        tone,
        exposure: d.scalar(51109, 0.)?,
        // DNG 1.4: 1 is None; 0 and values the specification reserves are Auto.
        black_render: if d.scalar(51110, 0.)? == 1. {
            super::BlackRender::None
        } else {
            super::BlackRender::Auto
        },
        dynamic_range: d.dynamic_range()?,
    };
    Ok(p)
}

/// The colour matrix of a profile at D65, interpolated between its two
/// calibration illuminants as `CameraProfile` does. A DNG that carries colour
/// matrices but no forward matrix has no profile `from_bytes` accepts, and this
/// is what it can still be rendered with. `None` when there is no matrix.
pub fn d65_color_matrix(b: &[u8]) -> Result<Option<Matrix>> {
    let d = Dcp::read(b)?;
    if !d.tags.contains_key(&50721) {
        return Ok(None);
    }
    let a = d.matrix(50721)?;
    let b = if d.tags.contains_key(&50722) {
        d.matrix(50722)?
    } else {
        a
    };
    let w = super::weight(6504., d.kelvin(50778)?, d.kelvin2()?);
    Ok(Some(std::array::from_fn(|i| {
        std::array::from_fn(|j| a[i][j] * (1. - w) + b[i][j] * w)
    })))
}

#[cfg(test)]
mod tests {
    /// A DCP holding ColorMatrix1 (identity) and CalibrationIlluminant1.
    fn dcp(illuminant: u16) -> Vec<u8> {
        let mut b = b"IIRC".to_vec();
        b.extend(8u32.to_le_bytes());
        let data = 8 + 2 + 2 * 12 + 4;
        b.extend(2u16.to_le_bytes());
        for (tag, kind, count, value) in [
            (50721u16, 10u16, 9u32, data as u32),
            (50778, 3, 1, u32::from(illuminant)),
        ] {
            b.extend(tag.to_le_bytes());
            b.extend(kind.to_le_bytes());
            b.extend(count.to_le_bytes());
            b.extend(value.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        for i in 0..9 {
            b.extend((if i % 4 == 0 { 1i32 } else { 0 }).to_le_bytes());
            b.extend(1i32.to_le_bytes());
        }
        b
    }
    #[test]
    fn flash_calibration_reads_as_daylight() {
        // Phase One IQ250's Adobe Standard is calibrated under flash.
        assert!(super::d65_color_matrix(&dcp(4)).unwrap().is_some());
        assert!(super::d65_color_matrix(&dcp(21)).unwrap().is_some());
        assert!(super::d65_color_matrix(&dcp(255)).is_err());
    }

    /// A DCP holding only tag 52551 with `kind`, `bytes` and `count`, in either byte
    /// order (the tag's own bytes are given as stored).
    fn dynamic_range_dcp(little: bool, kind: u16, count: u32, bytes: &[u8]) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut b = if little {
            b"II".to_vec()
        } else {
            b"MM".to_vec()
        };
        b.extend(u16b(0x4352));
        b.extend(u32b(8));
        b.extend(u16b(1));
        b.extend(u16b(52551));
        b.extend(u16b(kind));
        b.extend(u32b(count));
        b.extend(u32b(8 + 2 + 12 + 4));
        b.extend(u32b(0));
        b.extend(bytes);
        b
    }

    /// ProfileDynamicRange is read in the file's byte order and checked as the DNG
    /// specification lays it out; a zero hint is "no hint", not a ceiling.
    #[test]
    fn profile_dynamic_range_follows_the_files_byte_order_and_is_checked() {
        use super::{Dcp, DynamicRange};
        let read = |little, kind, count, bytes: &[u8]| {
            Dcp::read(&dynamic_range_dcp(little, kind, count, bytes))
                .unwrap()
                .dynamic_range()
        };
        let hint = 4f32;
        let mut be = vec![0, 1, 0, 1];
        be.extend(hint.to_be_bytes());
        assert_eq!(read(false, 7, 8, &be).unwrap(), DynamicRange::High);
        let mut le = vec![1, 0, 0, 0];
        le.extend(0f32.to_le_bytes());
        assert_eq!(read(true, 7, 8, &le).unwrap(), DynamicRange::Standard);
        // Read big-endian, these little-endian bytes are version 256.
        let error = read(false, 7, 8, &le).unwrap_err().to_string();
        assert!(error.contains("version 256"), "{error}");
        let mut range = vec![1, 0, 2, 0];
        range.extend(0f32.to_le_bytes());
        assert!(read(true, 7, 8, &range).is_err());
        let mut negative = vec![1, 0, 1, 0];
        negative.extend((-1f32).to_le_bytes());
        assert!(read(true, 7, 8, &negative).is_err());
        let mut nan = vec![1, 0, 1, 0];
        nan.extend(f32::NAN.to_le_bytes());
        assert!(read(true, 7, 8, &nan).is_err());
        // The wrong type or length.
        assert!(read(true, 1, 8, &le).is_err());
        assert!(read(true, 7, 6, &le[..6]).is_err());
    }
}
