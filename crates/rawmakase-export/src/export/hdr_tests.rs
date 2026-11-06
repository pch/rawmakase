//! HDR exports through the whole export path: the pixels as the `tiff` crate's
//! decoder reads them back and decoded by PQ's EOTF or read as float, the embedded
//! profile's CICP as moxcms reads it, and the refusals before rendering.
use super::srgb_fixtures::{photo, synthetic, text_watermark};
use crate::{
    camera_profiles::CameraProfile,
    color::hdr_output::{GRAPHICS_WHITE_NITS, HdrGamut, HdrOutput, HdrTransfer, pq_decode},
    export::{Replace, job},
    export_settings::{ExportSettings, Format, Include},
    model::recipe::Recipe,
};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};

fn settings(output: Option<HdrOutput>) -> ExportSettings {
    let mut s = ExportSettings {
        include: Include::AllExceptCameraRaw,
        ..Default::default()
    };
    s.choose_format(Format::Tiff);
    s.hdr = output;
    s
}

fn hdr_recipe(image: &crate::camera_data::CameraImage) -> Recipe {
    let mut r = Recipe {
        profile: CameraProfile::camera_matrix_default(&image.metadata).map(std::sync::Arc::new),
        sharpening: 0.,
        ..Default::default()
    };
    r.hdr.enabled = true;
    r.hdr.limit = 4.;
    r.exposure = 3.;
    r
}

fn export(
    photo: &job::Photo,
    settings: &ExportSettings,
    dir: &Path,
    name: &str,
) -> Result<PathBuf> {
    let target = dir.join(format!("{name}.tif"));
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

/// The TIFF's pixels as floats (16-bit as 0–1) and its ICC profile.
fn read(path: &Path) -> Result<(Vec<f32>, Vec<u8>)> {
    let mut d = tiff::decoder::Decoder::new(std::fs::File::open(path)?)?;
    let icc = d.get_tag_u8_vec(tiff::tags::Tag::IccProfile)?;
    let pixels = match d.read_image()? {
        tiff::decoder::DecodingResult::U16(v) => v.iter().map(|v| *v as f32 / 65535.).collect(),
        tiff::decoder::DecodingResult::F32(v) => v,
        _ => anyhow::bail!("unexpected sample type"),
    };
    Ok((pixels, icc))
}

/// A PQ export carries light above graphics white as absolute luminance, no channel
/// above the limit, and a profile whose CICP names Rec. 2020 PQ; a linear one the
/// same light as floats.
#[test]
fn an_hdr_tiff_keeps_highlights_above_white() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(96, 48);
    let recipe = hdr_recipe(&image);
    let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let pq = HdrOutput {
        gamut: HdrGamut::Rec2020,
        transfer: HdrTransfer::Pq,
    };
    let (pixels, icc) = read(&export(&photo, &settings(Some(pq)), dir.path(), "pq")?)?;
    let white: Vec<f32> = pixels
        .iter()
        .map(|v| pq_decode(*v) / GRAPHICS_WHITE_NITS)
        .collect();
    let peak = white.iter().copied().fold(0f32, f32::max);
    assert!(peak > 2. && peak <= 16.02, "{peak}");
    let profile =
        moxcms::ColorProfile::new_from_slice(&icc).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let cicp = profile.cicp.expect("cicp");
    assert_eq!(
        (
            cicp.color_primaries as u8,
            cicp.transfer_characteristics as u8
        ),
        (9, 16)
    );
    let linear = HdrOutput {
        transfer: HdrTransfer::Linear,
        ..pq
    };
    let (floats, _) = read(&export(
        &photo,
        &settings(Some(linear)),
        dir.path(),
        "linear",
    )?)?;
    let float_peak = floats.iter().copied().fold(0f32, f32::max);
    assert!((float_peak / peak - 1.).abs() < 0.01, "{float_peak} {peak}");
    let hlg = HdrOutput {
        transfer: HdrTransfer::Hlg,
        ..pq
    };
    let (signal, _) = read(&export(&photo, &settings(Some(hlg)), dir.path(), "hlg")?)?;
    assert!(signal.iter().any(|v| *v > 0.75) && signal.iter().all(|v| *v <= 1.));
    Ok(())
}

/// An HDR export needs an HDR edit, a TIFF and no watermark; each is refused before
/// anything is written.
#[test]
fn an_hdr_export_refuses_what_it_cannot_write() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(32, 24);
    let mut recipe = hdr_recipe(&image);
    let output = Some(HdrOutput::default());
    recipe.hdr.enabled = false;
    let off = photo(
        image.clone(),
        &dir.path().join("in.dng"),
        dir.path(),
        recipe.clone(),
    );
    let error = export(&off, &settings(output), dir.path(), "off").unwrap_err();
    assert!(error.to_string().contains("HDR off"), "{error}");
    recipe.hdr.enabled = true;
    let mut marked = photo(
        image.clone(),
        &dir.path().join("in.dng"),
        dir.path(),
        recipe.clone(),
    );
    marked.watermark = Some(text_watermark());
    assert!(export(&marked, &settings(output), dir.path(), "marked").is_err());
    let on = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let mut jpeg = settings(output);
    jpeg.choose_format(Format::Jpeg);
    assert!(export(&on, &jpeg, dir.path(), "jpeg").is_err());
    assert!(std::fs::read_dir(dir.path())?.all(|e| {
        e.map(|e| e.path().extension().is_none_or(|x| x != "tif"))
            .unwrap_or(true)
    }));
    Ok(())
}

/// An SDR export of an HDR edit is its SDR rendition: the edit with HDR off.
#[test]
fn an_sdr_export_of_an_hdr_edit_is_its_sdr_rendition() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(32, 24);
    let recipe = hdr_recipe(&image);
    let mut off = recipe.clone();
    off.hdr.enabled = false;
    let a = export(
        &photo(
            image.clone(),
            &dir.path().join("in.dng"),
            dir.path(),
            recipe,
        ),
        &settings(None),
        dir.path(),
        "a",
    )?;
    let b = export(
        &photo(image, &dir.path().join("in.dng"), dir.path(), off),
        &settings(None),
        dir.path(),
        "b",
    )?;
    assert_eq!(read(&a)?.0, read(&b)?.0);
    Ok(())
}
