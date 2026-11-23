//! Limit File Size To: the highest JPEG quality whose whole file fits a size.
use anyhow::Result;

/// A JPEG encoded to fit a size limit.
pub(super) struct Fitted {
    pub bytes: Vec<u8>,
    /// What the export says when even the lowest quality is larger than the limit.
    pub note: Option<String>,
}

/// The file `encode` makes at the highest quality (1–100) whose size is at most
/// `limit` bytes, or at quality 1, the smallest, with a note when none is.
///
/// A JPEG's size grows with its quality almost everywhere, so the search halves the
/// range each time; only a size it measured is ever taken as fitting, so where the
/// sizes don't grow steadily the result may be a step below the best, never above
/// the limit. Seven encodes at most.
pub(super) fn fit(limit: u64, mut encode: impl FnMut(u8) -> Result<Vec<u8>>) -> Result<Fitted> {
    let (mut low, mut high) = (1u8, 100u8);
    let mut best: Option<Vec<u8>> = None;
    let mut smallest: Option<Vec<u8>> = None;
    while low <= high {
        let quality = low + (high - low) / 2;
        let bytes = encode(quality)?;
        if bytes.len() as u64 <= limit {
            best = Some(bytes);
            low = quality + 1;
        } else {
            if quality == 1 {
                smallest = Some(bytes);
                break;
            }
            high = quality - 1;
        }
    }
    if let Some(bytes) = best {
        return Ok(Fitted { bytes, note: None });
    }
    // Nothing fitted, so the search went down to quality 1.
    let bytes = match smallest {
        Some(bytes) => bytes,
        None => encode(1)?,
    };
    let note = format!(
        "larger than the {} limit even at the lowest quality: exported at {}; make it smaller under Image Sizing",
        kilobytes(limit),
        kilobytes(bytes.len() as u64),
    );
    Ok(Fitted {
        bytes,
        note: Some(note),
    })
}

/// `bytes` in K of 1000 bytes, as Limit File Size To counts them, rounded up.
fn kilobytes(bytes: u64) -> String {
    format!("{} K", bytes.div_ceil(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in encoder whose file is `size(quality)` bytes, counting its calls.
    fn sized(size: impl Fn(u8) -> usize, calls: &mut Vec<u8>) -> impl FnMut(u8) -> Result<Vec<u8>> {
        move |q| {
            calls.push(q);
            Ok(vec![q; size(q)])
        }
    }

    #[test]
    fn takes_the_highest_quality_that_fits() -> Result<()> {
        let mut calls = Vec::new();
        let fitted = fit(5_000, sized(|q| usize::from(q) * 100, &mut calls))?;
        assert_eq!(fitted.bytes.len(), 5_000);
        assert_eq!(fitted.bytes[0], 50);
        assert!(fitted.note.is_none());
        assert!(calls.len() <= 7, "{calls:?}");
        Ok(())
    }

    #[test]
    fn a_limit_above_the_best_quality_keeps_100() -> Result<()> {
        let mut calls = Vec::new();
        let fitted = fit(1_000_000, sized(|q| usize::from(q) * 100, &mut calls))?;
        assert_eq!(fitted.bytes[0], 100);
        assert!(fitted.note.is_none());
        Ok(())
    }

    #[test]
    fn exports_the_smallest_with_a_note_when_nothing_fits() -> Result<()> {
        let mut calls = Vec::new();
        let fitted = fit(500, sized(|q| 1_000 + usize::from(q), &mut calls))?;
        assert_eq!(fitted.bytes[0], 1);
        assert_eq!(
            fitted.note.as_deref(),
            Some(
                "larger than the 1 K limit even at the lowest quality: exported at 2 K; make it smaller under Image Sizing"
            )
        );
        assert_eq!(calls.last(), Some(&1));
        assert_eq!(calls.iter().filter(|&&q| q == 1).count(), 1);
        Ok(())
    }

    /// Where a higher quality happens to be smaller, the search still never takes a
    /// file over the limit.
    #[test]
    fn never_exceeds_the_limit_where_sizes_dip() -> Result<()> {
        let size = |q: u8| usize::from(q) * 100 - if q == 60 { 2_000 } else { 0 };
        for limit in (100..=10_000).step_by(37) {
            let mut calls = Vec::new();
            let fitted = fit(limit, sized(size, &mut calls))?;
            assert!(fitted.bytes.len() as u64 <= limit, "{limit}");
        }
        Ok(())
    }

    /// A noisy image, which JPEG can't shrink much without lowering its quality.
    fn noisy(width: u32, height: u32) -> crate::rendered::Rendered {
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 1000) as f32 / 999.
        };
        crate::rendered::Rendered {
            width,
            height,
            pixels: (0..width * height)
                .map(|_| [next(), next(), next()])
                .collect(),
        }
    }

    fn export_jpeg(
        dir: &std::path::Path,
        name: &str,
        options: &crate::export_settings::ExportOptions,
    ) -> Result<(u64, Option<String>)> {
        let source = dir.join("source.ARW");
        std::fs::write(&source, b"source")?;
        let target = dir.join(name);
        let note = super::super::export(
            &target,
            &source,
            &noisy(400, 300),
            &crate::camera_data::Metadata::default(),
            options,
            super::super::Replace::NoClobber,
        )?;
        Ok((std::fs::metadata(&target)?.len(), note))
    }

    /// Limit File Size To in the Export dialog's settings becomes a byte limit for a
    /// JPEG only, in K of 1000 bytes, and stands in for the quality.
    #[test]
    fn the_dialogs_limit_is_for_jpeg_in_thousands_of_bytes() {
        use crate::export_settings::{ExportSettings, Format};
        let mut s = ExportSettings {
            limit_size: true,
            size_limit: 250,
            ..Default::default()
        };
        assert_eq!(s.options().max_bytes, Some(250_000));
        s.choose_format(Format::Jxl);
        assert_eq!(s.options().max_bytes, None);
        s.choose_format(Format::Jpeg);
        s.limit_size = false;
        assert_eq!(s.options().max_bytes, None);
        // Settings saved before the choice read with it off.
        let read: ExportSettings = serde_json::from_str(r#"{"quality":80}"#).unwrap();
        assert!(!read.limit_size);
        assert_eq!(read.options().max_bytes, None);
    }

    /// The file, metadata included, fits the limit at a quality below the one
    /// chosen, which the limit replaces.
    #[test]
    fn a_jpeg_export_fits_its_size_limit() -> Result<()> {
        use crate::export_settings::ExportOptions;
        let dir = tempfile::tempdir()?;
        let (full, _) = export_jpeg(
            dir.path(),
            "full.jpg",
            &ExportOptions {
                quality: 100,
                ..Default::default()
            },
        )?;
        let (lowest, _) = export_jpeg(
            dir.path(),
            "lowest.jpg",
            &ExportOptions {
                quality: 1,
                ..Default::default()
            },
        )?;
        let limit = (full + lowest) / 2;
        let (size, note) = export_jpeg(
            dir.path(),
            "limited.jpg",
            &ExportOptions {
                quality: 100,
                max_bytes: Some(limit),
                ..Default::default()
            },
        )?;
        assert!(size <= limit, "{size} > {limit}");
        assert!(size > lowest, "{size} is no better than the lowest quality");
        assert_eq!(note, None);
        image::open(dir.path().join("limited.jpg"))?;
        Ok(())
    }

    /// A limit even the lowest quality can't meet exports the smallest file, as
    /// Lightroom does, and says so.
    #[test]
    fn an_unreachable_limit_exports_the_smallest_jpeg_and_says_so() -> Result<()> {
        use crate::export_settings::ExportOptions;
        let dir = tempfile::tempdir()?;
        let (lowest, _) = export_jpeg(
            dir.path(),
            "lowest.jpg",
            &ExportOptions {
                quality: 1,
                ..Default::default()
            },
        )?;
        let (size, note) = export_jpeg(
            dir.path(),
            "limited.jpg",
            &ExportOptions {
                max_bytes: Some(1_000),
                ..Default::default()
            },
        )?;
        assert_eq!(size, lowest);
        let note = note.expect("a note");
        assert!(note.starts_with("larger than the 1 K limit"), "{note}");
        Ok(())
    }

    /// A gain-map JPEG counts its whole file, the map included, and stays a gain-map
    /// JPEG whose index finds the map.
    #[test]
    fn a_gain_map_jpeg_fits_its_size_limit() -> Result<()> {
        use crate::{
            color::hdr_output::{HdrGamut, HdrOutput},
            export::{
                job,
                srgb_fixtures::{photo, synthetic},
            },
            export_settings::{ExportSettings, Format},
            model::recipe::Recipe,
        };
        let dir = tempfile::tempdir()?;
        let image = synthetic(384, 256);
        let mut recipe = Recipe {
            profile: crate::camera_profiles::CameraProfile::camera_matrix_default(&image.metadata)
                .map(std::sync::Arc::new),
            sharpening: 0.,
            ..Default::default()
        };
        recipe.hdr.enabled = true;
        recipe.hdr.limit = 4.;
        recipe.exposure = 3.;
        let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
        let mut s = ExportSettings::default();
        s.choose_format(Format::Jpeg);
        s.hdr = Some(HdrOutput {
            gamut: HdrGamut::Rec709,
            gain_map: true,
            ..Default::default()
        });
        s.quality = 100;
        let run = |s: &ExportSettings, name: &str| -> Result<(Vec<u8>, Option<String>)> {
            let target = dir.path().join(name);
            let note = job::run(
                photo.clone(),
                s,
                &target,
                super::super::Replace::NoClobber,
                rawmakase_cancel::Cancel::NEVER,
                |_| {},
            )?;
            Ok((std::fs::read(&target)?, note))
        };
        let (full, _) = run(&s, "full.jpg")?;
        s.limit_size = true;
        s.size_limit = (full.len() as u32 * 7 / 10).div_ceil(1000);
        let limit = u64::from(s.size_limit) * 1000;
        assert!(limit < full.len() as u64);
        let (bytes, note) = run(&s, "limited.jpg")?;
        assert!(bytes.len() as u64 <= limit, "{} > {limit}", bytes.len());
        assert_eq!(note, None);
        assert!(bytes.windows(4).any(|w| w == b"MPF\0"));
        image::load_from_memory(&bytes)?;
        Ok(())
    }
}
