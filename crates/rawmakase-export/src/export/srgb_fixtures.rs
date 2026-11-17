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
        "a5a6badc1db38036997b5a05f8c7b9c56dc862394dfe4bd65797ba944eafbac9",
    ),
    (
        "plain.tif",
        "9267a9dfb2fdb55e8f969ae592f11951c79535c84aa8aafd8680c16a768c34df",
    ),
    (
        "plain.png",
        "7aaff78d6e5fb0f8b500c1d183bc90f4c7dc84fd366b55feab3f6fe52adabd44",
    ),
    (
        "resized.jpg",
        "e05bd963c4e1e63491e7493b44ad82a24c6ac2a24ce52612b1e4cad899f49777",
    ),
    (
        "resized.tif",
        "a1d2b4a9639e2765ad2100f162aa7ba57111b3c61db6d1721b17497d51053153",
    ),
    (
        "resized.png",
        "8cd1b92cad6407b12bbf6f8c239cc121a5723f20d3ebffc2b0f2af9caff49580",
    ),
    (
        "sharpened.jpg",
        "ebe1182b9af3c1d3b0e9d44d0c12df28b003dc6db57dc66976ca310bc214b9c6",
    ),
    (
        "sharpened.tif",
        "27630a43fffcfc60663f8c907e324d0844b8f2e24fa0221d2d894ede037e031a",
    ),
    (
        "sharpened.png",
        "9876ff68759504d611943ed1b163c8e5f3fd04ab436ba0d6a9147a59a92515e9",
    ),
    (
        "local-detail.jpg",
        "10454048bf28eb59800a0a0b9892632ca6ba8dd33876c98cd7879d7ae6784399",
    ),
    (
        "local-detail.tif",
        "4b710f36e8bb48392030fa6f8af92d6c00a0e3b8f925a254057523e7927aa04b",
    ),
    (
        "local-detail.png",
        "3f71f951cf19cddbbce4411193bbe645dac7d0561c1107cd11d78a543fcafcc2",
    ),
    (
        "grain.jpg",
        "199cb4aea2144d741177206678d88c345976cc18825bcb8dc50678ee67a1519a",
    ),
    (
        "grain.tif",
        "c543fbae1571dcf04ccd7bfc24ca0f22cf9655f77d772e6595337346adae7f03",
    ),
    (
        "grain.png",
        "c2b64d80d8a83e9cc0ec503c0a5de3161c89c3a3a80d1ce7a8ae01df76a8ce18",
    ),
    (
        "vignette-highlight.jpg",
        "9c5cef9e958c761d157e7513276514cd8c8fb47161baeb3b8bf68ef0be60458d",
    ),
    (
        "vignette-highlight.tif",
        "d9783925c3fb504a72ede5da84387ca034c806d9049823e2f21606edfc7f454b",
    ),
    (
        "vignette-highlight.png",
        "3c3a1c8b5275ec34460c6902af0713c61b213039c13ab52bc0d3c84a62b4ac29",
    ),
    (
        "vignette-color.jpg",
        "20918094dda4d654da7b6dd3decbe243b85bdb4edc083827ff83b0855d532e63",
    ),
    (
        "vignette-color.tif",
        "31be16ccd5c32f4f8754039f88607bd05268dcadd892180a97ea8a937dc8b6c1",
    ),
    (
        "vignette-color.png",
        "970f224472a9af86f23a57da6506177e303518d2205c1345adbb65281a0128e9",
    ),
    (
        "vignette-paint.jpg",
        "ecd5a3555ba3eed1252a565ecb1c86f0e370506cc6613dc11e2d9af03159974f",
    ),
    (
        "vignette-paint.tif",
        "097a1d155c91ca336da2f02e088a6da5b44654efdcced485f572b7f0c53b9bf2",
    ),
    (
        "vignette-paint.png",
        "fd66457179603826dedcc76133c26f2509289d7d0152b434a27f9688b44146e2",
    ),
    (
        "watermark.jpg",
        "96ca9c0806124d3a6b847a9b9f0eb9e1c1541e627d5672eac8f982b174d5de1d",
    ),
    (
        "watermark.tif",
        "2a9fd8cdd60b18b204de8d75d8802e4a74e28c946a3fc863b309b512fd4aff3d",
    ),
    (
        "watermark.png",
        "f347b7035799026b34065baa64f569caab09b20cffe8d983ef3cb5236d393e1d",
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
        ..crate::watermark::fonts::test_watermark()
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
        "ba5eb056b2062f077c1a6d605caa0e26230613e718be020fca9b4470194a9bc5",
    ),
    (
        "graphic.tif",
        "f1a5c950ce8b71df3023dcf41831a6cbd799cb4999a810f4e0db610490027d58",
    ),
    (
        "graphic.png",
        "d24a1f315d8f2f1dd85f0a6dc59660127569d502a17ed7aeff91cee9b4195497",
    ),
];
