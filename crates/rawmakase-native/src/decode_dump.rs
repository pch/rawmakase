//! Develop fingerprints (`rawmakase decode-dump`): what each decode mode makes
//! of a RAW file, as the renderer receives it, so a change of decoder shows up
//! as a changed image. Every mode is decoded from a freshly opened file with no
//! decode cache; the pixels are hashed exactly, and those of a full decode that
//! is not a mosaic (small RAWs, which are converted from YCbCr in floating
//! point) can be written out whole for a per-pixel comparison.
use crate::camera_data::{CameraImage, Decode, Demosaic};
use serde::Serialize;
use sha2::Digest;
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Serialize)]
pub struct Dump {
    pub path: std::path::PathBuf,
    pub decoder: String,
    /// SHA-256 of the file's bytes; `None` when it cannot be read.
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the full decode demosaics a mosaic (`None` when unpacking fails).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mosaic: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<Mode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half: Option<Mode>,
}

/// One decode mode's image, or its error.
#[derive(Debug, Default, Serialize)]
pub struct Mode {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_factor: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_clipped: Option<u32>,
    /// SHA-256 of the pixels as little-endian f32, three per pixel, row by row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixels_sha256: Option<String>,
    /// SHA-256 of the image's metadata: as saved (JSON), then what saving
    /// leaves out (`fingerprint::Unsaved`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_sha256: Option<String>,
    /// Per channel, the mean value: a scale change shows here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<[f64; 3]>,
}

/// The develop fingerprint of the file at `path`. With `floats`, a full decode
/// that is not a mosaic is also written there as `<sha256>.full.f32` (raw
/// little-endian floats, three per pixel; the size is in the fingerprint).
pub fn dump(path: &Path, floats: Option<&Path>) -> Dump {
    let d = Dump {
        path: path.to_path_buf(),
        decoder: crate::raw::version(),
        sha256: None,
        error: None,
        mosaic: None,
        full: None,
        half: None,
    };
    let before = crate::fingerprint::identity(path);
    let mut d = describe(path, floats, d);
    // The file is read several times (hashed, then decoded per mode): a file
    // replaced in between would pair one file's hash with another's images.
    if before.is_none() || crate::fingerprint::identity(path) != before {
        if let (Some(dir), Some(sha256)) = (floats, &d.sha256) {
            let _ = std::fs::remove_file(dir.join(format!("{sha256}.full.f32")));
        }
        d.mosaic = None;
        d.full = None;
        d.half = None;
        d.error
            .get_or_insert_with(|| "The file changed while it was read".into());
    }
    d
}

fn describe(path: &Path, floats: Option<&Path>, mut d: Dump) -> Dump {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            d.error = Some(e.to_string());
            return d;
        }
    };
    let sha256 = hex(&sha2::Sha256::digest(&bytes));
    d.sha256 = Some(sha256.clone());
    drop(bytes);
    let mosaic = crate::photo::open(path).and_then(|raw| Ok(raw.sensor()?.channels == 1));
    match mosaic {
        Ok(mosaic) => d.mosaic = Some(mosaic),
        Err(e) => {
            d.error = Some(format!("{e:#}"));
            return d;
        }
    }
    let decode = |mode: Decode| {
        crate::photo::open(path).and_then(|raw| raw.develop(mode, &AtomicBool::new(false)))
    };
    let full = decode(Decode::Full(Demosaic::Rawmakase));
    if let (Some(dir), Ok(image), Some(false)) = (floats, &full, d.mosaic) {
        let file = dir.join(format!("{sha256}.full.f32"));
        if let Err(e) = std::fs::write(&file, bytemuck::cast_slice::<_, u8>(&image.pixels)) {
            d.error = Some(format!("{}: {e}", file.display()));
        }
    }
    d.full = Some(mode(full));
    d.half = Some(mode(decode(Decode::Half)));
    d
}

fn mode(image: anyhow::Result<CameraImage>) -> Mode {
    let image = match image {
        Ok(image) => image,
        Err(e) => {
            return Mode {
                error: Some(format!("{e:#}")),
                ..Default::default()
            };
        }
    };
    let mut hash = sha2::Sha256::new();
    let mut sum = [0f64; 3];
    for p in &image.pixels {
        for c in 0..3 {
            hash.update(p[c].to_le_bytes());
            sum[c] += f64::from(p[c]);
        }
    }
    let n = image.pixels.len().max(1) as f64;
    // Everything the renderer reads: the saved fields and those saving leaves out.
    let mut metadata = serde_json::to_vec(&image.metadata).unwrap_or_default();
    metadata.extend(
        serde_json::to_vec(&crate::fingerprint::Unsaved::of(&image.metadata)).unwrap_or_default(),
    );
    Mode {
        error: None,
        size: Some([image.width, image.height]),
        scale_factor: Some(image.scale_factor),
        scale_clipped: Some(image.scale_clipped),
        pixels_sha256: Some(hex(&hash.finalize())),
        metadata_sha256: Some(hex(&sha2::Sha256::digest(metadata))),
        mean: Some(sum.map(|s| s / n)),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn chart_develops_in_both_modes() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let d = super::dump(chart.as_ref(), None);
        assert_eq!(d.mosaic, Some(true));
        let (full, half) = (d.full.unwrap(), d.half.unwrap());
        assert_eq!(full.size, Some([970, 742]));
        assert_eq!(half.size, Some([485, 371]));
        assert!(full.pixels_sha256.is_some() && full.error.is_none());
        let dir = tempfile::tempdir().unwrap();
        let missing = super::dump(&dir.path().join("missing.dng"), None);
        assert!(missing.sha256.is_none() && missing.error.is_some() && missing.full.is_none());
    }
}
