//! sRGB exports as they were before wide-gamut export (issue #604), recorded on
//! macOS arm64: the SHA-256 of each file's decoded pixels and its embedded ICC
//! profile. Default and explicitly chosen sRGB must keep writing exactly these.
//!
//! The render calls the platform's `powf` and `exp`, which other systems' libm
//! may round an ulp differently, so the recorded values are checked on macOS
//! arm64 only; elsewhere the cases still run and their exports are checked
//! against each other.
use crate::{
    camera_data::{CameraImage, Demosaic, Metadata},
    decode_cache::DecodeCache,
    export::{Replace, job},
    export_settings::{ExportSettings, Format},
    model::{
        effects::VignetteStyle,
        masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape},
        recipe::Recipe,
    },
};
use anyhow::Result;
use image::ImageDecoder;
use rawmakase_cancel::Cancel;
use std::{path::Path, sync::Arc};

/// The formats with recorded fixtures and an `image` decoder; JPEG XL exports are
/// checked through jxl-oxide in `hdr_tests.rs`.
const FIXTURE_FORMATS: [Format; 3] = [Format::Jpeg, Format::Tiff, Format::Png];

/// The SHA-256 of `assets/srgb.icc`, which every sRGB export embeds.
const SRGB_ICC: &str = "d68baed8428c7b500c33a60da52a08a845b4bf4ae4745cbda046811e2b0eb3c9";

/// The SHA-256 of the decoded pixels, per case and format.
const RECORDED: &[(&str, &str)] = &[
    (
        "plain.jpg",
        "e468a0b28c16b38c043a2fbc47f4e0076254e52ef0c5f4db69444faa281014b9",
    ),
    (
        "plain.tif",
        "ac0d7e6429b895d493ae3f91a741185debf4bd89a4a17c9a2ce0b16b99457a38",
    ),
    (
        "plain.png",
        "4260d7075e713f6313635c9bd2c232c3b90ea4f0cb38e0f72aa659dc5df894e7",
    ),
    (
        "resized.jpg",
        "d62295148a64dfb733fb8484fd733a3380e4061a643aa8797fcc916fd100c798",
    ),
    (
        "resized.tif",
        "cdc48210bfbc6fd3e7bc375d2d14718868bdd4d207f85019ea1c1e6591abb2b7",
    ),
    (
        "resized.png",
        "bb2ea3fd010c83ae3cc236fbb85faa8cbc5426c840c4b56c36bb6a3328dab7de",
    ),
    (
        "sharpened.jpg",
        "fbd282edc9710cc028130a1735187bf384081f25f0b96baeb7199412c5e9dad2",
    ),
    (
        "sharpened.tif",
        "bfe3fd1a011ed397ec3b34145d44d93637800ca55c63a406ffb49c3726b59653",
    ),
    (
        "sharpened.png",
        "4c2f3d4c4ee5af94994b5392fdfcb37883657166674fa0f7cf9b4e903f59a916",
    ),
    (
        "local-detail.jpg",
        "d4ed1c8aae85a41c9d8b5e00990443cf1408aeb0f1996dfb749625a7f9d85231",
    ),
    (
        "local-detail.tif",
        "77c34130a6da5dddd7b1e0835ce3df74192653a3a34b263f1bf30724383ca462",
    ),
    (
        "local-detail.png",
        "b3ec5b9844e31542f2bda940a908bd9ef6a2e8efe273b272410e76a0e23d67f3",
    ),
    (
        "grain.jpg",
        "d11081ad73e58962a7cdc106a3cb68d4404535177ef5a983f2ea12999a07254e",
    ),
    (
        "grain.tif",
        "6a9d1ac1cd8adf99215c07c3229e8eef153675071c442fab1943bdaf522dcc01",
    ),
    (
        "grain.png",
        "4485a381378e687e972efb6393346bcc3bcb155ed5947742e5649f7969ca808b",
    ),
    (
        "vignette-highlight.jpg",
        "f624ed1544c450968b243d0d3b9370b39c3f7d801de3590b5f4ece4a913ce6a5",
    ),
    (
        "vignette-highlight.tif",
        "57c56f0f0f4fb4c40b267c4448dc1ee082e67760609bde32355a1b9aba5b6d4d",
    ),
    (
        "vignette-highlight.png",
        "75cb2ad9763a33eff4ac08da5714c3613868639c65c063898488203ff69af348",
    ),
    (
        "vignette-color.jpg",
        "e667900b2096574e5f114d1c191d4b0fc570f39735bb80c06699b8287d0e787c",
    ),
    (
        "vignette-color.tif",
        "4f1356e8254beacf22dff31377290b063894f0e8baf391a208aa174306d480ec",
    ),
    (
        "vignette-color.png",
        "ea2b7791cb675888d4c1a37563c22d0b5d3dd17c24b5ae809dc276bdd0aa8d6d",
    ),
    (
        "vignette-paint.jpg",
        "177af70d67e790b9023c88d1493cd8dfcc678845cd983a18a288a033f3984845",
    ),
    (
        "vignette-paint.tif",
        "7da553eaff05544be2c430805f7ac65d55bd857c6d932250deec7f585940776a",
    ),
    (
        "vignette-paint.png",
        "8f86ef19ad72586b9ba812d66b635606905769f6bb045a649c9440238003b27a",
    ),
    (
        "watermark.jpg",
        "42376dd0a42edf699f36c174f8109028da6628fb04a2f1bbabbfa32136d1a2aa",
    ),
    (
        "watermark.tif",
        "af191b6f19dbea143bf178250e4c900291a2ec97cc516cb6e1f1a7b0cda39aa7",
    ),
    (
        "watermark.png",
        "416b045ef92e43e266e0401be24bf4fa6d9ae964203f7ef05f015253793fa9f1",
    ),
    (
        "chart.jpg",
        "62efe1893f4bf10bce81cd2d98fadfe2d470a64e89e81c6efd8cbd0c75c1237f",
    ),
    (
        "chart.tif",
        "88d4603b7251251275bd0f0dd3cc437f669b613b92c4d7168a9785fc8bccf852",
    ),
    (
        "chart.png",
        "e3bbc1a4ddb8084b59c1211e0b4cead10bb7bf461e73af9b5463c44e160087f1",
    ),
];

/// A synthetic photo with saturated colors and smooth ramps, in a camera whose
/// matrix is the identity.
pub(super) fn synthetic(width: u32, height: u32) -> CameraImage {
    let metadata = Metadata {
        make: "Synthetic".into(),
        model: "Ramp".into(),
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let pixels = (0..width * height)
        .map(|i| {
            let (x, y) = (i % width, i / width);
            let t = x as f32 / (width - 1) as f32;
            let band = y * 6 / height;
            let level = 0.02 + 0.9 * t;
            match band {
                0 => [level, level * 0.08, level * 0.04],
                1 => [level * 0.05, level, level * 0.1],
                2 => [level * 0.03, level * 0.06, level],
                3 => [level, level * 0.7, 0.01],
                4 => [0.01, level * 0.8, level],
                _ => [level; 3],
            }
        })
        .collect();
    CameraImage {
        width,
        height,
        recovered: Default::default(),
        pixels,
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    }
}

pub(super) fn photo(image: CameraImage, source: &Path, cache: &Path, recipe: Recipe) -> job::Photo {
    job::Photo {
        image: Arc::new(image),
        source: source.to_path_buf(),
        recipe,
        values: Default::default(),
        watermark: None,
        watermarks: Default::default(),
        demosaic: Demosaic::default(),
        cache: Some(DecodeCache::new(cache.to_path_buf(), u64::MAX, "test")),
        lenses: crate::photo::LensProfiles::none(),
        ai: crate::ai_denoise::AiService::refusing("no AI Denoise here"),
        file: None,
    }
}

/// A mask over the right half that sharpens and smooths there.
fn local_detail(r: &mut Recipe) {
    r.masks.push(MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.4, 0.5],
            to: [0.6, 0.5],
        })],
        adjust: LocalAdjust {
            sharpness: 0.8,
            noise: 0.6,
            ..Default::default()
        },
        ..Default::default()
    });
}

/// A translucent red text mark with a shadow.
pub(super) fn text_watermark() -> crate::watermark::Watermark {
    crate::watermark::Watermark {
        name: "fixture".into(),
        text: "RAWmakase".into(),
        color: [0.9, 0.1, 0.1],
        opacity: 0.6,
        shadow: crate::watermark::Shadow {
            enabled: true,
            ..Default::default()
        },
        size: crate::watermark::Size::Proportional(0.6),
        ..crate::watermark::fonts::test_watermark()
    }
}

type Case = (&'static str, u32, fn(&mut Recipe), bool);

/// The recipe changes, export long edge (0 for full size) and whether a text
/// watermark goes on top.
pub(super) const CASES: [Case; 9] = [
    ("plain", 0, |_| {}, false),
    ("resized", 48, |_| {}, false),
    ("sharpened", 0, |r| r.sharpening = 0.8, false),
    ("local-detail", 0, local_detail, false),
    ("grain", 0, |r| r.effects.grain = 0.5, false),
    (
        "vignette-highlight",
        0,
        |r| {
            r.effects.vignette = -0.6;
            r.effects.vignette_highlights = 0.5;
        },
        false,
    ),
    (
        "vignette-color",
        0,
        |r| {
            r.effects.vignette = -0.5;
            r.effects.vignette_style = VignetteStyle::ColorPriority;
            r.effects.vignette_highlights = 0.4;
        },
        false,
    ),
    (
        "vignette-paint",
        0,
        |r| {
            r.effects.vignette = 0.5;
            r.effects.vignette_style = VignetteStyle::PaintOverlay;
        },
        false,
    ),
    ("watermark", 0, |_| {}, true),
];

/// The SHA-256 of `bytes`, in hexadecimal.
fn sha256(bytes: &[u8]) -> String {
    let sha: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(bytes).into();
    sha.iter().map(|b| format!("{b:02x}")).collect()
}

/// The decoded pixels of the export at `path` and its ICC profile. A TIFF's
/// profile is read with `tiff`: `image` 0.25.10's TIFF decoder finds none in
/// some small files that have one.
pub(super) fn decoded(path: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let icc = if Format::from_path(path) == Some(Format::Tiff) {
        tiff::decoder::Decoder::new(std::fs::File::open(path)?)?
            .get_tag_u8_vec(tiff::tags::Tag::IccProfile)?
    } else {
        decoder.icc_profile()?.unwrap_or_default()
    };
    let image = image::DynamicImage::from_decoder(decoder)?;
    Ok((image.into_bytes(), icc))
}

/// The SHA-256 of the decoded pixels of the export at `path` and of its ICC
/// profile.
pub(super) fn fingerprint(path: &Path) -> Result<(String, String)> {
    let (pixels, icc) = decoded(path)?;
    Ok((sha256(&pixels), sha256(&icc)))
}

/// Every case's fingerprint in every format, through the export job with
/// `settings` (its format replaced by each in turn).
pub(super) fn fingerprints(
    settings: &ExportSettings,
    dir: &Path,
) -> Result<Vec<(String, String, String)>> {
    let source = dir.join("source.dng");
    std::fs::write(&source, b"synthetic")?;
    let mut out = Vec::new();
    for (name, edge, edit, watermark) in CASES {
        let mut recipe = Recipe::default();
        edit(&mut recipe);
        let mut photo = photo(synthetic(96, 64), &source, &dir.join("cache"), recipe);
        if watermark {
            photo.watermark = Some(text_watermark());
        }
        for format in FIXTURE_FORMATS {
            let mut settings = settings.clone();
            settings.choose_format(format);
            settings.resize = edge > 0;
            settings.long_edge = edge.max(1);
            let target = dir.join(format!("{name}.{}", format.extension()));
            job::run(
                photo.clone(),
                &settings,
                &target,
                Replace::NoClobber,
                Cancel::NEVER,
                |_| {},
            )?;
            let (pixels, icc) = fingerprint(&target)?;
            out.push((format!("{name}.{}", format.extension()), pixels, icc));
        }
    }
    // The synthetic chart DNG, decoded and rendered with its embedded profile.
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let raw = crate::photo::open(&chart, &crate::photo::LensProfiles::none())?;
    let image = raw.develop(
        crate::camera_data::Decode::full(Default::default()),
        Cancel::NEVER,
    )?;
    let recipe = Recipe::for_metadata(&image.metadata);
    let chart_photo = photo(image, &chart, &dir.join("cache"), recipe);
    for format in FIXTURE_FORMATS {
        let mut settings = settings.clone();
        settings.choose_format(format);
        settings.resize = true;
        settings.long_edge = 200;
        let target = dir.join(format!("chart.{}", format.extension()));
        job::run(
            chart_photo.clone(),
            &settings,
            &target,
            Replace::NoClobber,
            Cancel::NEVER,
            |_| {},
        )?;
        let (pixels, icc) = fingerprint(&target)?;
        out.push((format!("chart.{}", format.extension()), pixels, icc));
    }
    Ok(out)
}

/// Checks `found` against `recorded`: the sRGB profile everywhere, the pixels
/// on macOS arm64. Prints what was found, as a recording is made.
pub(super) fn check_recorded(found: &[(String, String, String)], recorded: &[(&str, &str)]) {
    for (name, pixels, _) in found {
        println!("    (\"{name}\", \"{pixels}\"),");
    }
    for (name, _, icc) in found {
        assert_eq!(icc, SRGB_ICC, "{name} embeds another ICC profile");
    }
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return;
    }
    let names = |rows: &mut dyn Iterator<Item = &str>| rows.map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(
        names(&mut found.iter().map(|(n, ..)| n.as_str())),
        names(&mut recorded.iter().map(|(n, _)| *n)),
        "the cases recorded"
    );
    for ((name, pixels, _), (_, recorded)) in found.iter().zip(recorded) {
        assert_eq!(pixels, recorded, "{name}'s pixels changed");
    }
}

/// The default export settings (sRGB) write the recorded files.
#[test]
fn default_srgb_exports_match_the_recorded_fixtures() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let found = fingerprints(&ExportSettings::default(), dir.path())?;
    check_recorded(&found, RECORDED);
    Ok(())
}

/// sRGB chosen in the settings writes the recorded files too.
#[test]
fn chosen_srgb_exports_match_the_recorded_fixtures() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut settings = ExportSettings::default();
    settings.choose_color_space(crate::color::OutputSpace::Srgb);
    let found = fingerprints(&settings, dir.path())?;
    check_recorded(&found, RECORDED);
    Ok(())
}

/// A graphic watermark's pixels, translucent, over a plain export.
#[test]
fn a_graphic_watermark_over_an_srgb_export_matches_its_fixture() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let images = dir.path().join("images");
    std::fs::create_dir_all(&images)?;
    let logo = image::RgbaImage::from_fn(24, 12, |x, y| {
        image::Rgba([(x * 10) as u8, 200, (y * 20) as u8, 160])
    });
    logo.save(images.join("logo.png"))?;
    let mark = crate::watermark::Watermark {
        name: "logo".into(),
        style: crate::watermark::Style::Graphic,
        image: Some("logo.png".into()),
        opacity: 0.7,
        ..crate::watermark::fonts::test_watermark()
    };
    let ready = mark.ready_in(&images, &Default::default())?;
    let image = synthetic(96, 64);
    let mut rendered = crate::develop::ExportImage::srgb(crate::develop::render(
        &image,
        &Recipe::default().checked()?,
        0,
        Cancel::NEVER,
    )?);
    assert!(ready.apply(&mut rendered));
    let source = dir.path().join("source.dng");
    std::fs::write(&source, b"synthetic")?;
    let mut found = Vec::new();
    for format in FIXTURE_FORMATS {
        let target = dir.path().join(format!("graphic.{}", format.extension()));
        super::export_image(
            &target,
            &source,
            &rendered,
            &image.metadata,
            &Default::default(),
            &Default::default(),
            Replace::NoClobber,
        )?;
        let (pixels, icc) = fingerprint(&target)?;
        found.push((format!("graphic.{}", format.extension()), pixels, icc));
    }
    check_recorded(&found, RECORDED_GRAPHIC);
    Ok(())
}

/// [`RECORDED`] of the graphic watermark's exports.
const RECORDED_GRAPHIC: &[(&str, &str)] = &[
    (
        "graphic.jpg",
        "30c8b42e8c1dde691430e48b167ececedae642d29b99acf7dc30cff87d747ddd",
    ),
    (
        "graphic.tif",
        "64b5980d6a3abdb9dfd1d0c8f9e4c021e5ae31f7e5aa8899195dc29515936c94",
    ),
    (
        "graphic.png",
        "4ce87b552c999fc5bae9f03936e8844fb26154b7704f2202a484f73eb3c3d177",
    ),
];
