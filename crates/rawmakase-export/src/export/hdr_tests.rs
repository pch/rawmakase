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
        gain_map: false,
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

/// An HDR JPEG XL export decodes in jxl-oxide with its header's PQ encoding and the
/// same light as the TIFF; linear light is TIFF only, and an SDR JPEG XL is sRGB.
#[test]
fn an_hdr_jpeg_xl_carries_the_same_light_as_the_tiff() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(64, 32);
    let recipe = hdr_recipe(&image);
    let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let pq = HdrOutput {
        gamut: HdrGamut::DisplayP3,
        transfer: HdrTransfer::Pq,
        gain_map: false,
    };
    let tiff = export(&photo, &settings(Some(pq)), dir.path(), "pq")?;
    let mut jxl_settings = settings(Some(pq));
    jxl_settings.choose_format(Format::Jxl);
    let target = dir.path().join("pq.jxl");
    job::run(
        photo.clone(),
        &jxl_settings,
        &target,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let bytes = std::fs::read(&target)?;
    let decoded = jxl_oxide::JxlImage::builder()
        .read(std::io::Cursor::new(&bytes))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let encoding = format!("{:?}", decoded.image_header().metadata.colour_encoding);
    assert!(
        encoding.contains("P3") && encoding.contains("Pq"),
        "{encoding}"
    );
    let frame = decoded
        .render_frame(0)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let signal: Vec<u16> = frame
        .image_all_channels()
        .buf()
        .iter()
        .map(|v| (v * 65535.).round() as u16)
        .collect();
    let (from_tiff, _) = read(&tiff)?;
    let tiff_signal: Vec<u16> = from_tiff
        .iter()
        .map(|v| (v * 65535.).round() as u16)
        .collect();
    assert_eq!(
        signal, tiff_signal,
        "lossless: the JPEG XL holds the TIFF's samples"
    );
    let mut linear = jxl_settings.clone();
    linear.choose_hdr(Some(HdrOutput {
        transfer: HdrTransfer::Linear,
        ..pq
    }));
    assert!(linear.hdr_output().is_err());
    // An SDR JPEG XL holds exactly the PNG's 8-bit sRGB samples.
    let mut sdr = jxl_settings;
    sdr.choose_hdr(None);
    let sdr_jxl = dir.path().join("sdr.jxl");
    job::run(
        photo.clone(),
        &sdr,
        &sdr_jxl,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let mut png = sdr.clone();
    png.choose_format(Format::Png);
    let sdr_png = dir.path().join("sdr.png");
    job::run(
        photo,
        &png,
        &sdr_png,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let decoded = jxl_oxide::JxlImage::builder()
        .read(std::io::Cursor::new(std::fs::read(&sdr_jxl)?))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let encoding = format!("{:?}", decoded.image_header().metadata.colour_encoding);
    assert!(encoding.contains("Srgb"), "{encoding}");
    let frame = decoded
        .render_frame(0)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let samples: Vec<u8> = frame
        .image_all_channels()
        .buf()
        .iter()
        .map(|v| (v * 255.).round() as u8)
        .collect();
    let png_samples = image::open(&sdr_png)?.to_rgb8().into_raw();
    assert_eq!(samples, png_samples);
    sdr.choose_color_space(crate::color::OutputSpace::AdobeRgb);
    assert!(sdr.output_space().is_err());
    Ok(())
}

/// An HDR export never uses the SDR colour space: one left from a TIFF export
/// (Adobe RGB) doesn't stop an HDR JPEG XL. The target's format is checked against
/// the transfer before anything renders: linear light into a .jxl is refused.
#[test]
fn an_hdr_export_checks_its_own_encoding_in_the_targets_format() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(32, 24);
    let recipe = hdr_recipe(&image);
    let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let mut s = settings(Some(HdrOutput::default()));
    s.choose_color_space(crate::color::OutputSpace::AdobeRgb);
    s.choose_format(Format::Jxl);
    assert!(s.check_output().is_ok());
    let target = dir.path().join("kept.jxl");
    job::run(
        photo.clone(),
        &s,
        &target,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    assert!(target.exists());
    let mut linear = settings(Some(HdrOutput {
        transfer: HdrTransfer::Linear,
        ..Default::default()
    }));
    linear.choose_format(Format::Tiff);
    let wrong = dir.path().join("linear.jxl");
    let error = job::run(
        photo,
        &linear,
        &wrong,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("Linear"), "{error}");
    assert!(!wrong.exists());
    Ok(())
}

/// A gain-map JPEG: its base is the SDR rendition (the plain SDR JPEG export's
/// pixels, as `image` decodes both), its MPF index finds the gain map, and the map
/// with its ISO 21496-1 metadata turns the base back into the master's light.
#[test]
fn a_gain_map_jpeg_rebuilds_the_master_from_its_sdr_base() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(96, 48);
    let recipe = hdr_recipe(&image);
    let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let gain = HdrOutput {
        gamut: HdrGamut::Rec709,
        gain_map: true,
        ..Default::default()
    };
    let mut s = settings(Some(gain));
    s.choose_format(Format::Jpeg);
    s.quality = 100;
    let target = dir.path().join("gm.jpg");
    job::run(
        photo.clone(),
        &s,
        &target,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let bytes = std::fs::read(&target)?;
    // The MPF index: the second entry's size and offset from the TIFF header.
    let mpf = bytes.windows(4).position(|w| w == b"MPF\0").expect("MPF");
    let tiff = mpf + 4;
    let field = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let (size, offset) = (field(tiff + 50 + 20), field(tiff + 50 + 24));
    let map_jpeg = &bytes[tiff + offset..];
    assert_eq!(map_jpeg.len(), size);
    assert!(map_jpeg.starts_with(&[0xff, 0xd8]));
    let iso = map_jpeg
        .windows(28)
        .position(|w| w == b"urn:iso:std:iso:ts:21496:-1\0")
        .expect("ISO 21496-1 metadata");
    let meta = &map_jpeg[iso + 28..];
    let rational = |at: usize| {
        i32::from_be_bytes(meta[at..at + 4].try_into().unwrap()) as f32
            / u32::from_be_bytes(meta[at + 4..at + 8].try_into().unwrap()) as f32
    };
    let (min, max) = (rational(21), rational(29));
    assert!(
        (rational(13) - 4.).abs() < 1e-3,
        "the alternate's headroom is the limit"
    );
    // The base is the SDR rendition, as the SDR JPEG export writes it.
    let mut sdr = settings(None);
    sdr.choose_format(Format::Jpeg);
    sdr.quality = 100;
    let plain = dir.path().join("sdr.jpg");
    job::run(
        photo,
        &sdr,
        &plain,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let base = image::load_from_memory(&bytes)?.to_rgb8();
    assert_eq!(base.as_raw(), image::open(&plain)?.to_rgb8().as_raw());
    // Applied at full weight, the map brings back light above white.
    let map = image::load_from_memory(map_jpeg)?.to_luma8();
    let k = 1. / 64.;
    let peak = base
        .pixels()
        .zip(map.pixels())
        .map(|(b, g)| {
            let lin = crate::color::srgb_decode(b.0[1] as f32 / 255.);
            let gain = (min + (max - min) * g.0[0] as f32 / 255.).exp2();
            (lin + k) * gain - k
        })
        .fold(0f32, f32::max);
    assert!(peak > 2., "{peak}");
    Ok(())
}

/// A gain-map JPEG XL: its base decodes (jxl-oxide, which leaves the `jhgm` box
/// alone) as the SDR JPEG XL export's pixels, and its `jhgm` box holds the ISO 21496-1
/// metadata and the map's codestream, which decodes to the map's size.
#[test]
fn a_gain_map_jpeg_xl_carries_its_base_and_map() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(48, 32);
    let recipe = hdr_recipe(&image);
    let photo = photo(image, &dir.path().join("in.dng"), dir.path(), recipe);
    let gain = HdrOutput {
        gamut: HdrGamut::Rec709,
        gain_map: true,
        ..Default::default()
    };
    let mut s = settings(Some(gain));
    s.choose_format(Format::Jxl);
    let target = dir.path().join("gm.jxl");
    job::run(
        photo.clone(),
        &s,
        &target,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let mut sdr = settings(None);
    sdr.choose_format(Format::Jxl);
    let plain = dir.path().join("sdr.jxl");
    job::run(
        photo,
        &sdr,
        &plain,
        Replace::NoClobber,
        Cancel::NEVER,
        |_| {},
    )?;
    let pixels = |path: &Path| -> Result<Vec<f32>> {
        let image = jxl_oxide::JxlImage::builder()
            .read(std::io::Cursor::new(std::fs::read(path)?))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let frame = image.render_frame(0).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(frame.image_all_channels().buf().to_vec())
    };
    assert_eq!(pixels(&target)?, pixels(&plain)?);
    // The boxes, by their 8-byte headers.
    let bytes = std::fs::read(&target)?;
    let mut at = 12;
    let mut bundle = None;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if &bytes[at + 4..at + 8] == b"jhgm" {
            bundle = Some(&bytes[at + 8..at + size]);
        }
        at += size;
    }
    let bundle = bundle.expect("jhgm box");
    let meta = u16::from_be_bytes([bundle[1], bundle[2]]) as usize;
    assert_eq!(meta, 61);
    let codestream = &bundle[3 + meta + 1 + 4..];
    let map = jxl_oxide::JxlImage::builder()
        .read(std::io::Cursor::new(codestream))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let size = &map.image_header().size;
    assert_eq!((size.width, size.height), (48, 32));
    Ok(())
}
