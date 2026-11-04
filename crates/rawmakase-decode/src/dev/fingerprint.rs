//! Decode fingerprints: what opening and unpacking a RAW file yields, as plain
//! values, so a decoder update or a change to opening photos shows up as a changed
//! field (`rawmakase fingerprint`, `scripts/corpus/dataset-fingerprints.py`).
//! The file is opened by [`crate::photo::open`] and unpacked as development
//! unpacks it; nothing is demosaiced, so every value is exact on any machine.
use serde::Serialize;
use std::path::Path;

/// The fingerprint format; 2 has the header, preview and sensor sections.
pub const VERSION: u32 = 2;

#[derive(Debug, Serialize)]
pub struct Fingerprint {
    pub path: std::path::PathBuf,
    pub version: u32,
    /// The decoder that read it, with its version.
    pub decoder: String,
    /// SHA-256 of the file's bytes; `None` when it cannot be read.
    pub sha256: Option<String>,
    /// The step that failed: `read`, `open` (the decoder, or a sensor RAWmakase
    /// does not develop) or `unpack`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What opening the file says, before the sensor data is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<Header>,
    /// The embedded preview chosen for thumbnails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<Preview>,
    /// What unpacking the sensor data yields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorData>,
}

/// What opening the file says: the decoder's own facts, and the photo's
/// metadata after `photo::open`'s DNG, RAF and lens additions, every field as
/// it is saved and what saving leaves out.
#[derive(Debug, Serialize)]
pub struct Header {
    pub facts: crate::raw::Facts,
    pub metadata: serde_json::Value,
    pub unsaved: Unsaved,
}

/// The metadata the renderer reads that is not saved with it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Unsaved {
    /// SHA-256 of the DNG's embedded camera profile, when it is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedded_dcp: Option<String>,
    /// The imported lens profiles that fit, and a SHA-256 of their identities
    /// and ranks in order.
    pub lens_profiles: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens_profiles_sha256: Option<String>,
}

impl Unsaved {
    pub fn of(m: &crate::camera_data::Metadata) -> Self {
        use sha2::Digest;
        let profiles = m.lens_profiles.all();
        let mut hash = sha2::Sha256::new();
        for c in profiles {
            hash.update(format!("{:?} {:?}\n", c.profile, c.lens_rank));
        }
        Self {
            embedded_dcp: m
                .embedded_dcp
                .as_deref()
                .map(|d| hex(&sha2::Sha256::digest(d))),
            lens_profiles: profiles.len(),
            lens_profiles_sha256: (!profiles.is_empty()).then(|| hex(&hash.finalize())),
        }
    }
}

/// The embedded JPEG preview, or why there is none.
#[derive(Debug, Serialize)]
pub struct Preview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The unpacked sensor data and its levels. Per-channel values are in the order
/// the recordings use (LibRaw's): red, green, blue, second green.
#[derive(Debug, Serialize)]
pub struct SensorData {
    /// 1 for a mosaic, 3 or 4 for small RAW files.
    pub channels: u32,
    /// Where the hashed area starts in the raw buffer (left, top), and its size.
    pub origin: [u32; 2],
    pub area: [u32; 2],
    pub black: [u32; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub black_pattern: Option<BlackPattern>,
    pub white: u32,
    pub linear_white: [u32; 4],
    /// The visible colour pattern's rows, e.g. `RG/gB` (`g` is the second green),
    /// `GGRGGB/…` for X-Trans; empty without a mosaic.
    pub cfa: String,
    /// SHA-256 of the unpacked raw data in the hashed area.
    pub data_sha256: String,
}

/// A black level that repeats over `rows × cols` pixels, on top of `black`.
#[derive(Debug, Serialize)]
pub struct BlackPattern {
    pub rows: u32,
    pub cols: u32,
    pub values: Vec<u32>,
}

/// The fingerprint of the file at `path`. A file that cannot be read, opened or
/// unpacked gives a fingerprint with its error, not an `Err`.
pub fn fingerprint(path: &Path) -> Fingerprint {
    let mut f = Fingerprint {
        path: path.to_path_buf(),
        version: VERSION,
        decoder: crate::raw::version(),
        sha256: None,
        failed: None,
        error: None,
        header: None,
        preview: None,
        sensor: None,
    };
    let before = identity(path);
    if let Err((step, e)) = describe(path, &mut f) {
        f.failed = Some(step);
        f.error = Some(format!("{e:#}"));
    }
    // The file is read twice (hashed, then decoded): a file replaced in between
    // would pair one file's hash with another's fingerprint.
    if before.is_some() && identity(path) != before {
        f.header = None;
        f.preview = None;
        f.sensor = None;
        f.failed = Some("read");
        f.error = Some("The file changed while it was read".into());
    }
    f
}

/// What tells one file at `path` from another: size, modification time and,
/// on Unix, device and inode.
pub(crate) fn identity(path: &Path) -> Option<(u64, std::time::SystemTime, u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    Some((m.len(), m.modified().ok()?, dev, ino))
}

fn describe(path: &Path, f: &mut Fingerprint) -> Result<(), (&'static str, anyhow::Error)> {
    use sha2::Digest;
    let read = |e: std::io::Error| ("read", e.into());
    f.sha256 = Some(hex(&sha2::Sha256::digest(
        std::fs::read(path).map_err(read)?,
    )));
    let mut raw = crate::photo::open(path).map_err(|e| ("open", e))?;
    f.header = Some(Header {
        facts: raw.facts.clone(),
        metadata: serde_json::to_value(&raw.metadata).map_err(|e| ("open", e.into()))?,
        unsaved: Unsaved::of(&raw.metadata),
    });
    let sensor = raw.sensor();
    // After unpacking, as the recordings were made.
    f.preview = Some(match raw.thumbnail() {
        Ok(jpeg) => Preview {
            size: Some(jpeg.len()),
            sha256: Some(hex(&sha2::Sha256::digest(&jpeg))),
            error: None,
        },
        Err(e) => Preview {
            size: None,
            sha256: None,
            error: Some(format!("{e:#}")),
        },
    });
    let s = sensor.map_err(|e| ("unpack", e))?;
    let (rows, cols, values) = s.black_pattern;
    f.sensor = Some(SensorData {
        channels: s.channels,
        origin: s.origin,
        area: s.area,
        black: s.black,
        black_pattern: (rows > 0 && cols > 0).then_some(BlackPattern { rows, cols, values }),
        white: s.white,
        linear_white: s.linear_white,
        cfa: cfa(&s.pattern),
        data_sha256: hex(&s.data_sha256),
    });
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The pattern's rows as letters, reduced to the period that repeats.
fn cfa(pattern: &[Vec<u8>]) -> String {
    let period = |n: usize, same: &dyn Fn(usize, usize) -> bool| {
        (1..=n)
            .find(|&p| n.is_multiple_of(p) && (p..n).all(|i| same(i, i % p)))
            .unwrap_or(n)
    };
    let rows = period(pattern.len(), &|a, b| pattern[a] == pattern[b]);
    let cols = pattern.first().map_or(0, |row| {
        period(row.len(), &|a, b| pattern.iter().all(|r| r[a] == r[b]))
    });
    pattern[..rows]
        .iter()
        .map(|row| {
            row[..cols]
                .iter()
                .map(|&c| match c {
                    0 => 'R',
                    1 => 'G',
                    3 => 'g',
                    2 => 'B',
                    _ => '?',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    #[test]
    fn chart_dng_fingerprint_is_pinned() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let mut json = serde_json::to_value(super::fingerprint(chart.as_ref())).unwrap();
        let fields = json.as_object_mut().unwrap();
        assert_eq!(fields.remove("path").unwrap(), chart);
        assert_eq!(fields.remove("decoder").unwrap(), crate::raw::version());
        // White balance and the camera matrix are computed in floating point,
        // which may round differently with another compiler; the rest is exact
        // everywhere.
        let header = fields.get_mut("header").unwrap().as_object_mut().unwrap();
        let metadata = header.get_mut("metadata").unwrap().as_object_mut().unwrap();
        metadata.remove("matrix").unwrap();
        for (field, want) in [
            ("wb", [1.8181818, 1., 1.3888888]),
            ("daylight_wb", [1.817948, 1., 1.3889403]),
        ] {
            let got: [f32; 3] = serde_json::from_value(metadata.remove(field).unwrap()).unwrap();
            assert!(
                got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-5),
                "{field}: {got:?}"
            );
        }
        // As the generator (tests/color/dng.rs) writes it: RGGB, black 0, white
        // 65535, no DefaultCrop (65535 marks none), no preview and a camera
        // Adobe does not know, whose colour is in the embedded profile; the
        // camera matrix is the file's D65 ColorMatrix.
        let want = serde_json::json!({
            "version": 2,
            "sha256": "3b23ed9df6b4e9ce0f504f8e01ad9b3c3d105d11bf177dac95f40a99f70a0407",
            "header": {
                "facts": {
                    "margins": [0, 0],
                    "frame": [65535, 65535, 0, 0],
                    "aspect": [65535, 65535, 0, 0],
                    "normalized": ["", "Synthetic Chart"],
                    "dng_version": 17039360,
                },
                "metadata": {
                    "aperture": 0.0,
                    "baseline_exposure": 0.0,
                    "cam_xyz": [[1.0800800323486328, -0.29205700755119324, -0.16179099678993225], [-0.6779360175132751, 1.6356430053710938, 0.021849999204277992], [0.0016240000259131193, -0.02789900079369545, 0.6954100131988525]],
                    "crop_height": 0,
                    "crop_left": 65535,
                    "crop_top": 65535,
                    "crop_width": 0,
                    "flip": 0,
                    "focal": 0.0,
                    "focal_35mm": 0.0,
                    "fuji_dynamic_range": 65535,
                    "height": 742,
                    "highlight_tone_priority": "Off",
                    "iso": 0.0,
                    "make": "RAWmakase",
                    "model": "Synthetic Chart",
                    "raw_height": 742,
                    "raw_width": 970,
                    "shutter": 0.0,
                    "width": 970,
                    "xtrans": false,
                },
                "unsaved": {
                    "embedded_dcp": "46fe3ae96e2c878860cd0ce061bad5ec18403c35732c479b3a6f02006e4ca0c9",
                    "lens_profiles": 0,
                },
            },
            "preview": {"error": "No thumbnail in file"},
            "sensor": {
                "channels": 1,
                "origin": [0, 0],
                "area": [970, 742],
                "black": [0, 0, 0, 0],
                "black_pattern": {"rows": 1, "cols": 1, "values": [0]},
                "white": 65535,
                "linear_white": [0, 0, 0, 0],
                "cfa": "RG/gB",
                "data_sha256": "d7f816bb902c0e23d349d53c86a0925200019bd37cf18bca732dfca370ca89e5",
            },
        });
        assert_eq!(json, want);
    }

    #[test]
    fn renderer_inputs_that_are_not_saved_change_the_fingerprint() {
        let mut m = crate::camera_data::Metadata::default();
        let none = super::Unsaved::of(&m);
        m.embedded_dcp = Some(std::sync::Arc::from(&b"profile"[..]));
        let one = super::Unsaved::of(&m);
        m.embedded_dcp = Some(std::sync::Arc::from(&b"another"[..]));
        let other = super::Unsaved::of(&m);
        assert!(none != one && one != other, "{none:?} {one:?} {other:?}");
    }

    #[test]
    fn unreadable_and_non_raw_files_are_errors_not_failures() {
        let dir = tempfile::tempdir().unwrap();
        let missing = super::fingerprint(&dir.path().join("missing.dng"));
        assert!(missing.sha256.is_none() && missing.failed == Some("read"));
        let text = dir.path().join("text.dng");
        std::fs::write(&text, b"not a raw file").unwrap();
        let f = super::fingerprint(&text);
        assert_eq!(
            f.sha256.as_deref(),
            Some("75d3499f372b9001c41b9ca9d7c9c5398477905e49e949746947ea7942b2f859")
        );
        assert!(f.failed == Some("open") && f.header.is_none() && f.sensor.is_none());
    }

    #[test]
    fn cfa_patterns_reduce_to_their_period() {
        let bayer: Vec<Vec<u8>> = (0..16)
            .map(|r| if r % 2 == 0 { vec![0, 1] } else { vec![3, 2] })
            .collect();
        assert_eq!(super::cfa(&bayer), "RG/gB");
        let xtrans = ["GGRGGB", "GGBGGR", "BRGRBG", "GGBGGR", "GGRGGB", "RBGBRG"];
        let pattern: Vec<Vec<u8>> = xtrans
            .iter()
            .map(|row| {
                row.bytes()
                    .map(|c| b"RGB".iter().position(|&x| x == c).unwrap() as u8)
                    .collect()
            })
            .collect();
        assert_eq!(super::cfa(&pattern), xtrans.join("/"));
        // A period of 4 rows inside LibRaw's 16 by 16 (Leaf) pattern.
        let leaf: Vec<Vec<u8>> = (0..16).map(|r| vec![(r % 4) as u8 % 3; 16]).collect();
        assert_eq!(super::cfa(&leaf), "R/G/B/R");
        assert_eq!(super::cfa(&[]), "");
    }
}
