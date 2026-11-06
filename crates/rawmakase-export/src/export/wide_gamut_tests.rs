//! Exports in Adobe RGB (1998) and ProPhoto RGB through the whole export path: the
//! pixels, as an ICC-aware reader that is not the encoder (moxcms) reads them back,
//! the embedded profile, EXIF's colour space, and the refusals before rendering.
use super::srgb_fixtures::{decoded, photo, synthetic, text_watermark};
use crate::{
    color::OutputSpace,
    export::{Replace, job},
    export_settings::{ExportSettings, Format, Include},
    model::recipe::Recipe,
};
use anyhow::Result;
use moxcms::{ColorProfile, Layout, TransformOptions};
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};

/// The formats with recorded fixtures and an `image` decoder; JPEG XL exports are
/// checked through jxl-oxide in `hdr_tests.rs`.
const FIXTURE_FORMATS: [Format; 3] = [Format::Jpeg, Format::Tiff, Format::Png];

fn settings(format: Format, space: OutputSpace) -> ExportSettings {
    let mut s = ExportSettings {
        include: Include::AllExceptCameraRaw,
        ..Default::default()
    };
    s.choose_format(format);
    s.choose_color_space(space);
    s
}

/// The export of `photo` with `settings` at `dir/name.<ext>`.
fn export(
    photo: &job::Photo,
    settings: &ExportSettings,
    dir: &Path,
    name: &str,
) -> Result<PathBuf> {
    let target = dir.join(format!("{name}.{}", settings.format.extension()));
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

/// The decoded pixels of `path`, 0–1, three a pixel.
fn pixels(path: &Path) -> Result<Vec<f64>> {
    let (bytes, _) = decoded(path)?;
    Ok(if Format::from_path(path) == Some(Format::Tiff) {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f64::from(u16::from_ne_bytes(*b)) / 65535.)
            .collect()
    } else {
        bytes.iter().map(|b| f64::from(*b) / 255.).collect()
    })
}

/// `path`'s pixels as encoded sRGB, through moxcms's transform from the profile the
/// file embeds to RAWmakase's sRGB profile. Its floating-point path: moxcms 0.8.1's
/// default fixed-point one reads ProPhoto's mid gray 0.5 back as 0.492.
fn as_srgb(path: &Path) -> Result<Vec<f64>> {
    let (_, icc) = decoded(path)?;
    let from = ColorProfile::new_from_slice(&icc).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let to = ColorProfile::new_from_slice(crate::color_management::srgb_profile())
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let transform = from
        .create_transform_f64(
            Layout::Rgb,
            &to,
            Layout::Rgb,
            TransformOptions {
                prefer_fixed_point: false,
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let src = pixels(path)?;
    let mut out = vec![0.; src.len()];
    transform
        .transform(&src, &mut out)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(out)
}

/// An Adobe RGB or ProPhoto export, read back through its own profile, is the sRGB
/// export wherever sRGB holds the colour. In 16-bit TIFF, grays within 4 code
/// values and colours within 3e-4 (a tenth of an 8-bit code value): the profiles'
/// s15.16 colorants and white differ from the published chromaticities, which the
/// conversion follows, by up to 2e-5, which a reader's matrix carries into the
/// darker channels of saturated colours. In 8-bit PNG, grays within 2.5 code values
/// and colours within 0.008 in linear light: a dark channel beside a bright one
/// is read back as the difference of two Adobe RGB values, each rounded to 8 bits.
#[test]
fn wide_gamut_exports_read_back_through_their_profiles_as_the_srgb_export() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.dng");
    std::fs::write(&source, b"synthetic")?;
    // Muted colours and a neutral ramp, all inside sRGB.
    let mut image = synthetic(96, 64);
    for p in &mut image.pixels {
        let mean = (p[0] + p[1] + p[2]) / 3.;
        *p = p.map(|v| mean + (v - mean) * 0.35);
    }
    // Without sharpening, whose luminance is each space's own (as the vignette's,
    // grain's and local Noise's are): the pixels differ there by design.
    let recipe = Recipe {
        sharpening: 0.,
        ..Default::default()
    };
    let photo = photo(image, &source, &dir.path().join("cache"), recipe);
    for (format, space, gray, colour) in [
        (Format::Tiff, OutputSpace::AdobeRgb, 4. / 65535., 3e-4),
        (Format::Tiff, OutputSpace::ProPhoto, 4. / 65535., 3e-4),
        (Format::Png, OutputSpace::AdobeRgb, 2.5 / 255., 0.008),
    ] {
        let srgb = pixels(&export(
            &photo,
            &settings(format, OutputSpace::Srgb),
            dir.path(),
            &format!("srgb-for-{space:?}"),
        )?)?;
        let wide = export(
            &photo,
            &settings(format, space),
            dir.path(),
            &format!("{space:?}"),
        )?;
        let back = as_srgb(&wide)?;
        let mut grays = 0;
        for (s, b) in srgb.chunks(3).zip(back.chunks(3)) {
            if !s.iter().all(|v| (0.002..0.998).contains(v)) {
                continue;
            }
            let is_gray = (s[0] - s[1]).abs() < 1e-4 && (s[1] - s[2]).abs() < 1e-4;
            grays += usize::from(is_gray);
            let close = |s: &f64, b: &f64| match (is_gray, format) {
                (true, _) | (false, Format::Tiff) => {
                    (s - b).abs() <= if is_gray { gray } else { colour }
                }
                (false, _) => {
                    let linear = |v: f64| f64::from(crate::color::srgb_decode(v as f32));
                    (linear(*s) - linear(*b)).abs() <= colour
                }
            };
            assert!(
                s.iter().zip(b).all(|(s, b)| close(s, b)),
                "{format:?} {space:?}: {s:?} read back as {b:?}"
            );
        }
        assert!(grays > 50, "{grays} grays");
    }
    Ok(())
}

/// A green sRGB can't hold, under a translucent red watermark and resized, comes out
/// of an Adobe RGB or ProPhoto TIFF outside sRGB, read back by moxcms; the opaque
/// part of the mark is the mark's sRGB red.
#[test]
fn wide_gamut_greens_and_a_watermark_survive_the_whole_export() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.dng");
    std::fs::write(&source, b"synthetic")?;
    // Band 1 of the synthetic photo is a saturated green.
    let mut photo = photo(
        synthetic(96, 64),
        &source,
        &dir.path().join("cache"),
        Recipe::default(),
    );
    let mut mark = text_watermark();
    mark.text = "MW".into();
    mark.opacity = 1.;
    mark.shadow.enabled = false;
    mark.size = crate::watermark::Size::Proportional(0.5);
    photo.watermark = Some(mark);
    let mut extended = 0;
    for space in [OutputSpace::AdobeRgb, OutputSpace::ProPhoto] {
        let mut s = settings(Format::Tiff, space);
        (s.resize, s.long_edge) = (true, 72);
        let srgb = pixels(&export(
            &photo,
            &ExportSettings {
                resize: true,
                long_edge: 72,
                ..settings(Format::Tiff, OutputSpace::Srgb)
            },
            dir.path(),
            "srgb",
        )?)?;
        std::fs::remove_file(dir.path().join("srgb.tif"))?;
        let wide = export(&photo, &s, dir.path(), &format!("{space:?}"))?;
        // The linear sRGB the file describes, unclipped.
        let linear: Vec<[f64; 3]> = pixels(&wide)?
            .chunks(3)
            .map(|p| {
                let lin: [f64; 3] = std::array::from_fn(|c| f64::from(space.decode(p[c] as f32)));
                let m = crate::color::inverse(space.from_linear_srgb());
                std::array::from_fn(|r| (0..3).map(|c| f64::from(m[r][c]) * lin[c]).sum())
            })
            .collect();
        let (w, h) = (72usize, 48usize);
        assert_eq!(linear.len(), w * h);
        // The green band's middle row, from a third of the way in: sRGB clips its red.
        let row = h * 3 / 12;
        for x in w / 3..w * 2 / 3 {
            let s = &srgb[(row * w + x) * 3..][..3];
            let g = linear[row * w + x];
            if s[0] <= 0.001 && g[0] < -0.005 {
                extended += 1;
            }
        }
        // Where the sRGB export is the mark's red, so is this one, read back.
        let red = [0.9, 0.1, 0.1];
        let mut covered = 0;
        for (s, b) in srgb.chunks(3).zip(as_srgb(&wide)?.chunks(3)) {
            if s.iter().zip(red).all(|(s, r)| (s - r).abs() < 0.002) {
                covered += 1;
                assert!(
                    b.iter().zip(red).all(|(b, r)| (b - r).abs() < 0.004),
                    "{space:?}: {b:?}"
                );
            }
        }
        assert!(covered > 10, "{space:?}: the mark covers {covered} pixels");
    }
    assert!(extended > 20, "{extended} green pixels outside sRGB");
    Ok(())
}

/// A source JPEG whose camera EXIF says sRGB: ColorSpace 1 and index "R98".
fn srgb_camera_jpeg(path: &Path) -> Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    let block = super::exif::tiff_block_with(
        CameraExif {
            main: vec![
                Field::ascii(tag::MAKE, "Test"),
                Field::ascii(tag::MODEL, "Cam"),
            ],
            exif: vec![Field::short(tag::COLOR_SPACE, 1)],
            gps: Vec::new(),
        },
        &[Field::ascii(1, "R98")],
    );
    assert_eq!(
        crate::exif::colour_facts_of_jpeg(&jpeg_with_exif(&block))
            .map(|f| (f.color_space, f.interop_index)),
        Some((Some(1), Some("R98".into())))
    );
    std::fs::write(path, jpeg_with_exif(&block))?;
    Ok(())
}
fn jpeg_with_exif(block: &[u8]) -> Vec<u8> {
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend(((block.len() + 8) as u16).to_be_bytes());
    out.extend(b"Exif\0\0");
    out.extend(block);
    out.extend([0xff, 0xd9]);
    out
}

/// Each space's export embeds its profile and EXIF colour space (and Adobe RGB the
/// interoperability index R03) in every format it can be written in, never the
/// camera's sRGB tags, and keeps the profile with every metadata switch off.
#[test]
fn each_space_embeds_its_profile_and_exif_colour_space() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("camera.jpg");
    srgb_camera_jpeg(&source)?;
    let photo = photo(
        synthetic(32, 24),
        &source,
        &dir.path().join("cache"),
        Recipe::default(),
    );
    for space in OutputSpace::ALL {
        for format in FIXTURE_FORMATS {
            if format.supports(space).is_err() {
                continue;
            }
            for include in [Include::AllExceptCameraRaw, Include::CopyrightOnly] {
                let mut s = settings(format, space);
                s.include = include;
                let name = format!("{space:?}-{include:?}");
                let target = export(&photo, &s, dir.path(), &name)?;
                let (_, icc) = decoded(&target)?;
                assert_eq!(
                    icc,
                    crate::color_management::output_profile(space),
                    "{name} {format:?}"
                );
                let facts = crate::exif::colour_facts(&target)
                    .unwrap_or_else(|| panic!("{name} {format:?}: EXIF"));
                let expected = match space {
                    OutputSpace::Srgb => (Some(1), None),
                    OutputSpace::AdobeRgb => (Some(0xffff), Some("R03".to_string())),
                    OutputSpace::ProPhoto => (Some(0xffff), None),
                };
                assert_eq!(
                    (facts.color_space, facts.interop_index),
                    expected,
                    "{name} {format:?}"
                );
            }
        }
    }
    Ok(())
}

/// ProPhoto RGB in a JPEG or PNG, settings or target, is refused before anything
/// renders, and nothing is written; so is a colour space from a later release.
#[test]
fn an_export_that_cant_hold_its_colour_space_is_refused_before_rendering() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.dng");
    std::fs::write(&source, b"not a raw")?;
    // A draft: rendering would first decode the source, which fails differently.
    let mut image = synthetic(16, 12);
    image.fast = true;
    let photo = photo(image, &source, &dir.path().join("cache"), Recipe::default());
    let refused = |s: &ExportSettings, target: &Path| {
        let error = job::run(
            photo.clone(),
            s,
            target,
            Replace::NoClobber,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(!target.exists());
        error
    };
    for format in [Format::Jpeg, Format::Png] {
        let s = settings(format, OutputSpace::ProPhoto);
        let target = dir.path().join(format!("out.{}", format.extension()));
        let error = refused(&s, &target);
        assert!(
            error.contains("ProPhoto RGB") && error.contains("TIFF"),
            "{error}"
        );
    }
    // TIFF settings, a JPEG target (as a socket client names it).
    let error = refused(
        &settings(Format::Tiff, OutputSpace::ProPhoto),
        &dir.path().join("named.jpg"),
    );
    assert!(error.contains("ProPhoto RGB"), "{error}");
    // A space only a later release knows.
    let file = dir.path().join("export.json");
    std::fs::write(&file, r#"{"color_space":"DisplayP3","format":"Tiff"}"#)?;
    let later = ExportSettings::load_file(&file).unwrap();
    let error = refused(&later, &dir.path().join("later.tif"));
    assert!(error.contains("later version"), "{error}");
    // A batch with them plans nothing.
    let plan = super::batch::plan(&[], &settings(Format::Png, OutputSpace::ProPhoto), None);
    assert!(matches!(plan, Err(super::batch::Unplanned::Settings(_))));
    Ok(())
}

/// The staging of a ProPhoto image refuses a format that can't hold it, however it
/// was rendered.
#[test]
fn a_prophoto_image_is_never_staged_as_jpeg_or_png() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let image = synthetic(16, 12);
    let rendered = crate::develop::render_export(
        &image,
        &Recipe::default().checked()?,
        None,
        0,
        OutputSpace::ProPhoto,
        Cancel::NEVER,
    )?;
    for name in ["a.jpg", "a.png"] {
        let target = dir.path().join(name);
        let staged = super::stage_image(
            &target,
            &rendered,
            &image.metadata,
            &Default::default(),
            &Default::default(),
        );
        assert!(staged.is_err(), "{name}");
    }
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    let tif = dir.path().join("a.tif");
    super::export_image(
        &tif,
        &dir.path().join("source.dng"),
        &rendered,
        &image.metadata,
        &Default::default(),
        &Default::default(),
        Replace::NoClobber,
    )?;
    assert_eq!(
        decoded(&tif)?.1,
        crate::color_management::output_profile(OutputSpace::ProPhoto)
    );
    Ok(())
}

/// A TIFF export writes UNDEFINED fields as UNDEFINED, not BYTE: the camera's
/// ExifVersion and Adobe RGB's InteroperabilityVersion.
#[test]
fn a_tiff_export_keeps_undefined_fields_undefined() -> Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    use crate::tiff::kind::UNDEFINED;
    let dir = tempfile::tempdir()?;
    let image = synthetic(16, 12);
    let rendered = crate::develop::render_export(
        &image,
        &Recipe::default().checked()?,
        None,
        0,
        OutputSpace::AdobeRgb,
        Cancel::NEVER,
    )?;
    let embed = super::Embed {
        camera: Some(CameraExif {
            main: vec![Field::ascii(tag::MAKE, "Test")],
            exif: vec![Field {
                tag: 0x9000,
                kind: UNDEFINED,
                count: 4,
                bytes: b"0231".to_vec(),
            }],
            gps: Vec::new(),
        }),
        ..Default::default()
    };
    let tif = dir.path().join("a.tif");
    super::export_image(
        &tif,
        &dir.path().join("source.dng"),
        &rendered,
        &image.metadata,
        &Default::default(),
        &embed,
        Replace::NoClobber,
    )?;
    let mut t = crate::tiff::Tiff::open(std::fs::File::open(&tif)?, 0).unwrap();
    let main = t.ifd(t.first).unwrap();
    let exif = t.ifd(t.offset(&main[&tag::EXIF_IFD]).unwrap()).unwrap();
    let version = &exif[&0x9000];
    assert_eq!((version.kind, version.count), (UNDEFINED, 4));
    let interop = t
        .ifd(t.offset(&exif[&tag::INTEROPERABILITY_IFD]).unwrap())
        .unwrap();
    let version = &interop[&2];
    assert_eq!((version.kind, version.count), (UNDEFINED, 4));
    assert_eq!(t.raw(version).as_deref(), Some(&b"0100"[..]));
    Ok(())
}
