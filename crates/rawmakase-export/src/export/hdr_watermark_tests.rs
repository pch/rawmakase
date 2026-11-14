//! Watermarks in HDR exports, read back from each HDR format: the mark's white is
//! graphics white in the decoded light, pixels it doesn't cover are the unmarked
//! export's, and a gain map's base is the marked SDR export.
use super::srgb_fixtures::{photo, synthetic};
use crate::{
    camera_profiles::CameraProfile,
    color::hdr_output::{HdrGamut, HdrOutput, HdrTransfer},
    export::{Replace, job},
    export_settings::{ExportSettings, Format, Include},
    model::recipe::Recipe,
    watermark::Watermark,
};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};

const W: u32 = 160;
const H: u32 = 80;

/// White text, fully opaque, as wide as the photo: its stems are wide enough to have
/// pixels the mark covers completely.
fn white_mark() -> Watermark {
    Watermark {
        name: "white".into(),
        text: "IH".into(),
        color: [1.; 3],
        opacity: 1.,
        size: crate::watermark::Size::Fit,
        ..Default::default()
    }
}

/// The mark's alpha at each of the photo's pixels, as the export places it.
fn coverage() -> Result<Vec<f32>> {
    let mark = white_mark().ready()?.place(W, H).expect("a mark");
    let mut alpha = vec![0.; (W * H) as usize];
    for my in 0..mark.height {
        for mx in 0..mark.width {
            let (x, y) = (mark.x + mx as i64, mark.y + my as i64);
            if (0..i64::from(W)).contains(&x) && (0..i64::from(H)).contains(&y) {
                alpha[(y as u32 * W + x as u32) as usize] = mark.rgba[my * mark.width + mx][3];
            }
        }
    }
    Ok(alpha)
}

fn source(dir: &Path, watermark: Option<Watermark>) -> job::Photo {
    let image = synthetic(W, H);
    let mut recipe = Recipe {
        profile: CameraProfile::camera_matrix_default(&image.metadata).map(std::sync::Arc::new),
        sharpening: 0.,
        ..Default::default()
    };
    recipe.hdr.enabled = true;
    recipe.hdr.limit = 4.;
    recipe.exposure = 3.;
    let mut p = photo(image, &dir.join("in.dng"), dir, recipe);
    p.watermark = watermark;
    p
}

fn settings(format: Format, output: Option<HdrOutput>, lossy: bool) -> ExportSettings {
    let mut s = ExportSettings {
        include: Include::AllExceptCameraRaw,
        ..Default::default()
    };
    s.choose_format(format);
    s.hdr = output;
    s.jxl_lossy = lossy;
    s.quality = 100;
    s
}

fn export(
    photo: &job::Photo,
    settings: &ExportSettings,
    dir: &Path,
    name: &str,
) -> Result<PathBuf> {
    let target = dir.join(name);
    job::run(
        photo.clone(),
        settings,
        &target,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    Ok(target)
}

/// A TIFF's samples as floats: 16-bit ones as 0–1.
fn tiff_samples(path: &Path) -> Result<Vec<f32>> {
    let mut d = tiff::decoder::Decoder::new(std::fs::File::open(path)?)?;
    Ok(match d.read_image()? {
        tiff::decoder::DecodingResult::U16(v) => v.iter().map(|v| *v as f32 / 65535.).collect(),
        tiff::decoder::DecodingResult::F32(v) => v,
        _ => anyhow::bail!("unexpected sample type"),
    })
}

fn jxl_samples(path: &Path) -> Result<Vec<f32>> {
    let image = jxl_oxide::JxlImage::builder()
        .read(std::io::Cursor::new(std::fs::read(path)?))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let frame = image.render_frame(0).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(frame.image_all_channels().buf().to_vec())
}

/// Light (linear sRGB primaries, 1 = graphics white) of each pixel of `samples`
/// encoded for `output`.
fn light(samples: &[f32], output: HdrOutput) -> Vec<[f32; 3]> {
    let to_srgb = HdrGamut::Rec709.prophoto_to_rgb();
    samples
        .chunks(3)
        .map(|p| crate::color::mul(to_srgb, output.decode([p[0], p[1], p[2]])))
        .collect()
}

/// Checks marked against unmarked light: untouched where the mark has no alpha, graphics
/// white where it is opaque (within `tolerance` at the median, `worst` at any pixel),
/// where the unmarked export had light far above it.
fn check(
    name: &str,
    marked: &[[f32; 3]],
    plain: &[[f32; 3]],
    alpha: &[f32],
    exact_outside: bool,
    tolerance: f32,
    worst: f32,
) {
    let mut errors = Vec::new();
    let mut brightest_under = 0f32;
    for (i, a) in alpha.iter().enumerate() {
        if *a <= 0. {
            if exact_outside {
                assert_eq!(marked[i], plain[i], "{name}: pixel {i} outside the mark");
            }
        } else if *a >= 1. {
            brightest_under = brightest_under.max(plain[i][1]);
            errors.extend(marked[i].map(|v| (v - 1.).abs()));
        }
    }
    errors.sort_by(f32::total_cmp);
    assert!(errors.len() > 60, "{name}: {} opaque samples", errors.len());
    let (median, max) = (errors[errors.len() / 2], errors[errors.len() - 1]);
    assert!(
        median < tolerance && max < worst,
        "{name}: white off by {median} (median), {max} (most)"
    );
    assert!(brightest_under > 4., "{name}: {brightest_under}");
}

/// PQ, HLG and linear TIFFs, and lossless and lossy JPEG XL: the mark is graphics white
/// in the decoded light, over light up to 16× white, and the rest of a lossless file
/// is the unmarked one's, sample for sample.
#[test]
fn an_hdr_export_draws_its_watermark_at_graphics_white() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let alpha = coverage()?;
    let marked = source(dir.path(), Some(white_mark()));
    let plain = source(dir.path(), None);
    for (gamut, transfer) in [
        (HdrGamut::Rec2020, HdrTransfer::Pq),
        (HdrGamut::DisplayP3, HdrTransfer::Hlg),
        (HdrGamut::Rec709, HdrTransfer::Linear),
    ] {
        let output = HdrOutput {
            gamut,
            transfer,
            gain_map: false,
        };
        let name = format!("{}-{}", gamut.cicp(), transfer.cicp());
        let s = settings(Format::Tiff, Some(output), false);
        let ours = tiff_samples(&export(&marked, &s, dir.path(), &format!("{name}-m.tif"))?)?;
        let theirs = tiff_samples(&export(&plain, &s, dir.path(), &format!("{name}.tif"))?)?;
        check(
            &name,
            &light(&ours, output),
            &light(&theirs, output),
            &alpha,
            true,
            1e-3,
            1e-3,
        );
        if transfer == HdrTransfer::Linear {
            continue;
        }
        // Lossless: the TIFF's samples. Lossy: the mark's white within its coding.
        let s = settings(Format::Jxl, Some(output), false);
        let lossless = jxl_samples(&export(&marked, &s, dir.path(), &format!("{name}-m.jxl"))?)?;
        assert!(
            lossless
                .iter()
                .zip(&ours)
                .all(|(a, b)| (a - b).abs() < 1e-6),
            "{name}: lossless JPEG XL"
        );
        let s = settings(Format::Jxl, Some(output), true);
        let lossy = jxl_samples(&export(&marked, &s, dir.path(), &format!("{name}-l.jxl"))?)?;
        check(
            &format!("{name} lossy"),
            &light(&lossy, output),
            &light(&theirs, output),
            &alpha,
            false,
            0.05,
            0.5,
        );
    }
    Ok(())
}

/// A gain map's base is the marked SDR export, sample for sample, in JPEG and JPEG
/// XL, and the JPEG's map brings the mark back as graphics white: the map is computed
/// from a marked base and a marked master, so the mark is not lit to the master's
/// light beneath it.
#[test]
fn a_gain_map_export_marks_its_base_and_its_master() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let alpha = coverage()?;
    let marked = source(dir.path(), Some(white_mark()));
    let plain = source(dir.path(), None);
    let gain = HdrOutput {
        gamut: HdrGamut::Rec709,
        gain_map: true,
        ..Default::default()
    };
    for format in [Format::Jxl, Format::Jpeg] {
        let ext = if format == Format::Jpeg { "jpg" } else { "jxl" };
        let ours = export(
            &marked,
            &settings(format, Some(gain), false),
            dir.path(),
            &format!("gm.{ext}"),
        )?;
        let sdr = export(
            &marked,
            &settings(format, None, false),
            dir.path(),
            &format!("sdr.{ext}"),
        )?;
        if format == Format::Jxl {
            assert_eq!(jxl_samples(&ours)?, jxl_samples(&sdr)?);
            continue;
        }
        let bytes = std::fs::read(&ours)?;
        let base = image::load_from_memory(&bytes)?.to_rgb8();
        assert_eq!(base.as_raw(), image::open(&sdr)?.to_rgb8().as_raw());
        let (map, min, max) = jpeg_gain_map(&bytes)?;
        let k = 1. / 64.;
        let rebuilt: Vec<[f32; 3]> = base
            .pixels()
            .zip(map.pixels())
            .map(|(b, g)| {
                let gain = (min + (max - min) * g.0[0] as f32 / 255.).exp2();
                b.0.map(|v| (crate::color::srgb_decode(v as f32 / 255.) + k) * gain - k)
            })
            .collect();
        // The unmarked master's light under the mark, from the unmarked TIFF.
        let pq = HdrOutput {
            gamut: HdrGamut::Rec709,
            ..Default::default()
        };
        let unmarked = tiff_samples(&export(
            &plain,
            &settings(Format::Tiff, Some(pq), false),
            dir.path(),
            "plain.tif",
        )?)?;
        check(
            "gain-map JPEG",
            &rebuilt,
            &light(&unmarked, pq),
            &alpha,
            false,
            0.05,
            0.25,
        );
    }
    Ok(())
}

/// A gain-map JPEG's map (8-bit grayscale) and its ISO 21496-1 log2 minimum and
/// maximum, found through the base's MPF index.
fn jpeg_gain_map(bytes: &[u8]) -> Result<(image::GrayImage, f32, f32)> {
    let mpf = bytes.windows(4).position(|w| w == b"MPF\0").expect("MPF");
    let tiff = mpf + 4;
    let field = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let offset = field(tiff + 50 + 24);
    let map_jpeg = &bytes[tiff + offset..];
    let iso = map_jpeg
        .windows(28)
        .position(|w| w == b"urn:iso:std:iso:ts:21496:-1\0")
        .expect("ISO 21496-1 metadata");
    let meta = &map_jpeg[iso + 28..];
    let rational = |at: usize| {
        i32::from_be_bytes(meta[at..at + 4].try_into().unwrap()) as f32
            / u32::from_be_bytes(meta[at + 4..at + 8].try_into().unwrap()) as f32
    };
    Ok((
        image::load_from_memory(map_jpeg)?.to_luma8(),
        rational(21),
        rational(29),
    ))
}
