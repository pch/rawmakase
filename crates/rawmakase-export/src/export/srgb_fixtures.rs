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
        "2ff6671cd253a376a161d2022f40d125941258febe07a2967fe5d01556ff0766",
    ),
    (
        "plain.tif",
        "a09e53caa84275999b848ff3070ffaf3908916358c5175dd4b3782e7b2605078",
    ),
    (
        "plain.png",
        "2fb38039cc878150b0fdf459693eb7d039fab3cb659867c21c65c58d29496dce",
    ),
    (
        "resized.jpg",
        "a3ff764598097e2586dd02c416d389471fee7582285b1979eb959d7d99f81812",
    ),
    (
        "resized.tif",
        "0fe32667bf37cae82753841c3f0cfc2f23b4da2d5d255890cda9578fd08150d4",
    ),
    (
        "resized.png",
        "5069450ea3c9a0f1536f47ed4357ee108f5f1b8a6a1fe0924635cc808e840a56",
    ),
    (
        "sharpened.jpg",
        "4fc015d8fe9639884817252e2112a3bb66bbf3885677d9210df12f1b2b356a41",
    ),
    (
        "sharpened.tif",
        "548efbd5129856eaf1c6a078b8dedf38e6ad8b658bfe52e7b45b5f79c2bb4c85",
    ),
    (
        "sharpened.png",
        "9c9913b94dd8f59bf6959facc206f5270567a890bb625d23f204a0fcdb61cb2c",
    ),
    (
        "local-detail.jpg",
        "933e4fdac61be3cad13d789b824db8b3cac446e53f64385068e528a145e2207f",
    ),
    (
        "local-detail.tif",
        "08b4dd5ffd0690dedb03b02ed31efb7ff6c8b437b65520aa74e19becc2bf9d33",
    ),
    (
        "local-detail.png",
        "f9d930ba480e6f38d7a2ddaafa4b271444c1f7929fcbebd99f06fbd6c9bb74af",
    ),
    (
        "grain.jpg",
        "13c4553adf14ff5b7032cfeb2de627065981752faf5b7e89f40019cff2cd0763",
    ),
    (
        "grain.tif",
        "7d131ceccd69efde1956ec0172d45103c45ae738cd2e3e291e8d7aacb19edf26",
    ),
    (
        "grain.png",
        "caee105930d7f341b085cda3eebb20b1a1e11fc09fe1418e14ec2a62a69f26f4",
    ),
    (
        "vignette-highlight.jpg",
        "b1b936c09c719f9936bc2fcdbf52ee8927f84a0ebfa3bdde9dca6f416e495439",
    ),
    (
        "vignette-highlight.tif",
        "efbbfdc8f66ea138626da2785a65fa9e594db05a254c8c90eff023b41d86c01b",
    ),
    (
        "vignette-highlight.png",
        "d6031e943380e0fe5d7e635b0024ba14d41adcd79d6de2e2449592edad1b7405",
    ),
    (
        "vignette-color.jpg",
        "a7f196abc1f06e2569d2c8c6f10addeb1d640e3cfc632952220b450383427c68",
    ),
    (
        "vignette-color.tif",
        "14830ddbae52d088a546351d597125aa3637af3a695c2f1125b062b8cd6fdf4d",
    ),
    (
        "vignette-color.png",
        "871ff55f83169fa1fe76197213c26b74274d181568d14aee37ff9805fa63fadf",
    ),
    (
        "vignette-paint.jpg",
        "ba3e77deffb1da50717af5bf215cc51316257b0d6753c07f406c87a8080aefad",
    ),
    (
        "vignette-paint.tif",
        "4e4b1abecc23bbd34a66fa2bfcdda53f7ac2d6b7baf63ff93a90fb9c726e83aa",
    ),
    (
        "vignette-paint.png",
        "f0c66beaba0960825af7ad9abe6c58309b2e52b560fef2de3a12f3d231248d7b",
    ),
    (
        "watermark.jpg",
        "f165bff2ee5a790c63c267a2c90a595d65ce69ed11692869c2e12852a80f0eb0",
    ),
    (
        "watermark.tif",
        "a28867701d4fa31909e89853b93428e42a871ae490059d5a9020e8d8f84aa031",
    ),
    (
        "watermark.png",
        "635eac808c7c98070b68c58a9989d5412791a9911ac5acbe3db062e759ed5169",
    ),
    (
        "chart.jpg",
        "62efe1893f4bf10bce81cd2d98fadfe2d470a64e89e81c6efd8cbd0c75c1237f",
    ),
    (
        "chart.tif",
        "972cc5807c23ce1fd78ba7c66136121e7e919dd993813548341e77a26bb7ff7a",
    ),
    (
        "chart.png",
        "e3bbc1a4ddb8084b59c1211e0b4cead10bb7bf461e73af9b5463c44e160087f1",
    ),
];

/// A synthetic photo with saturated colours and smooth ramps, in a camera whose
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
        demosaic: Demosaic::default(),
        cache: DecodeCache::new(cache.to_path_buf(), u64::MAX),
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
        ..Default::default()
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
    let raw = crate::photo::open(&chart)?;
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
        ..Default::default()
    };
    let ready = mark.ready_in(&images)?;
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
        "7ebd22c965c251be8a1039554cb4baa6ad3af1b731c179bbfb7b033bd9e50383",
    ),
    (
        "graphic.tif",
        "e5685fa2ad80db89e78bfa110948d4b994187a40e652b8ac55d3919d186b1023",
    ),
    (
        "graphic.png",
        "855c93549367f00b7451cdb7cf48611493f0db35fdec229799d5b0e949d4f86d",
    ),
];
