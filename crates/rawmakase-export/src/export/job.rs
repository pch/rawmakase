//! One photo's export from start to finish: the full-size image (decoded here when
//! only a quick preview is loaded), the render, its metadata and the file.
use super::{
    Embed,
    assemble::{Policy, Values, assemble, copyright},
};
use crate::export_settings::ExportSettings;
use crate::{
    camera_data::{CameraImage, Demosaic},
    decode::{DecodePolicy, FullSize},
    decode_cache::DecodeCache,
    exif,
    model::recipe::Recipe,
    raw::Raw,
};
use anyhow::{Result, ensure};
use rawmakase_cancel::Cancel;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// The photo as it was when Export was chosen.
#[derive(Clone)]
pub struct Photo {
    pub image: Arc<CameraImage>,
    pub source: PathBuf,
    pub recipe: Recipe,
    /// Its catalog metadata: rating, label, keywords and descriptive fields.
    pub values: Values,
    /// The watermark chosen in the Export dialog, a preset or the Simple
    /// Copyright Watermark.
    pub watermark: Option<crate::watermark::Watermark>,
    /// The demosaic for a full-size decode, if `image` is a draft.
    pub demosaic: Demosaic,
    /// Where a full-size decode is looked up first.
    pub cache: DecodeCache,
    /// Where the AI Denoise source comes from when the edit has AI Denoise on.
    pub ai: crate::ai_denoise::AiService,
    /// The file as it was before `image` was decoded from it, for AI Denoise: a file
    /// changed since fails the export rather than mixing two files, and so does one
    /// whose identity could not be read (`None`).
    pub file: Option<crate::storage::Identity>,
    /// `image` is an editable preview: reduced (`fast`, which sizes radii for the
    /// original), yet the image to render. It is never replaced by a full-size
    /// decode, renders without AI Denoise, as Develop shows it (an editable preview
    /// has no sensor data), and nothing is read from `source`, which may be offline
    /// or another file by now: the camera's EXIF stands in from `image`'s metadata.
    pub from_editable_preview: bool,
}

/// Exports `photo` to `target`, reporting progress from 0 to 1. Stops between
/// stages once `cancel` is set. Returns what the export has to say besides
/// "Exported", such as a Simple Copyright Watermark left out for want of a
/// copyright.
pub fn run(
    photo: Photo,
    settings: &ExportSettings,
    target: &Path,
    replace: super::Replace,
    cancel: Cancel<'_>,
    progress: impl Fn(f32),
) -> Result<Option<String>> {
    // The target's format, not only the settings', must hold the colour space or the
    // HDR encoding; checked before anything renders.
    let format = crate::export_settings::Format::from_path(target);
    if let Some(output) = settings.hdr_output()? {
        let format = format.ok_or_else(|| {
            anyhow::anyhow!("An HDR export is a TIFF or JPEG XL: name it .tif, .tiff or .jxl")
        })?;
        crate::export_settings::Format::check_hdr(format, output)?;
    } else if let Some(format) = format {
        format.supports(settings.output_space()?)?;
    }
    let prepared = prepare(&photo, settings, cancel, &progress)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    super::export_pixels(
        target,
        &photo.source,
        &prepared.rendered,
        &prepared.metadata,
        &prepared.options,
        &prepared.embed,
        replace,
    )?;
    progress(1.);
    Ok(prepared.notice)
}

/// A photo rendered for export, with what goes in the file beside its pixels.
pub struct Prepared {
    /// The pixels, in the colour space or HDR encoding the settings chose.
    pub rendered: super::Pixels,
    pub metadata: crate::camera_data::Metadata,
    pub options: crate::export_settings::ExportOptions,
    pub embed: Embed,
    /// What the export has to say besides "Exported".
    pub notice: Option<String>,
}

/// Renders `photo` for an export with `settings`, reporting progress up to 0.85.
/// Stops between stages once `cancel` is set. Settings whose colour space can't be
/// exported (in their format, or from a later release) are refused first.
pub fn prepare(
    photo: &Photo,
    settings: &ExportSettings,
    cancel: Cancel<'_>,
    progress: &impl Fn(f32),
) -> Result<Prepared> {
    settings.check_output()?;
    let hdr = settings.hdr_output()?;
    if hdr.is_some() {
        // Captured with the job: the edit's own intent, never the editor's or display's.
        ensure!(
            photo.recipe.hdr.is_active(),
            "This photo's edit has HDR off: turn on High Dynamic Range in Develop, or export it in SDR"
        );
        ensure!(
            photo.watermark.is_none(),
            "Watermarks are not supported in HDR exports yet"
        );
    }
    let cancelled = || -> Result<()> {
        ensure!(!cancel.is_cancelled(), "Cancelled");
        Ok(())
    };
    progress(0.05);
    let policy = Policy::of(settings);
    let simple_copyright = matches!(
        &photo.watermark,
        Some(w) if w.name == crate::watermark::SIMPLE_COPYRIGHT
    );
    // Read once: for the export's metadata and the Simple Copyright
    // Watermark.
    let file = (!photo.from_editable_preview && (policy.reads_file() || simple_copyright))
        .then(|| crate::photo::camera_exif(&photo.source))
        .flatten();
    // Loaded first: a missing image or font fails the export before it
    // renders.
    let mut notice = None;
    let watermark = match &photo.watermark {
        Some(_) if simple_copyright => match copyright(&photo.values, file.as_ref()) {
            Some(text) => Some(crate::watermark::Watermark::simple_copyright(&text).ready()?),
            None => {
                notice = Some("no copyright in the file for the Simple Copyright Watermark".into());
                None
            }
        },
        Some(w) => Some(w.ready()?),
        None => None,
    };
    let image = if photo.from_editable_preview {
        photo.image.clone()
    } else {
        full_size(
            photo.image.clone(),
            &photo.source,
            photo.demosaic,
            &photo.cache,
            cancel,
        )?
    };
    cancelled()?;
    progress(0.4);
    let options = settings.options();
    let ai = ai_source(photo, &image, cancel)?;
    cancelled()?;
    let recipe = photo.recipe.checked()?;
    let rendered = match hdr {
        Some(output) => {
            let master =
                crate::develop::render_hdr(&image, &recipe, ai.as_ref(), options.max_edge, cancel)?;
            if output.gain_map {
                // The base is the edit's SDR rendition, rendered as an sRGB export.
                let base = crate::develop::render_export(
                    &image,
                    &recipe,
                    ai.as_ref(),
                    options.max_edge,
                    crate::color::OutputSpace::Srgb,
                    cancel,
                )?;
                super::Pixels::GainMap(super::GainMapPixels::new(base, &master)?)
            } else {
                super::Pixels::Hdr(super::HdrPixels::encode(&master, output))
            }
        }
        None => {
            let mut rendered = crate::develop::render_export(
                &image,
                &recipe,
                ai.as_ref(),
                options.max_edge,
                settings.output_space()?,
                cancel,
            )?;
            if let Some(w) = &watermark
                && !w.apply(&mut rendered)
            {
                notice = Some("the watermark's text has no characters its font can draw".into());
            }
            super::Pixels::Sdr(rendered)
        }
    };
    cancelled()?;
    progress(0.85);
    let file = file.filter(|_| policy.reads_file());
    let libraw = file.is_none().then(|| {
        if photo.from_editable_preview {
            exif::CameraExif::from_libraw(&image.metadata)
        } else {
            stand_in(&photo.source, &image.metadata)
        }
    });
    let assembled = assemble(policy, file.as_ref(), libraw, &photo.values);
    let xmp = assembled
        .xmp
        .as_ref()
        .map(|fields| xmp(photo, &image, settings, fields));
    Ok(Prepared {
        rendered,
        metadata: image.metadata.clone(),
        options,
        embed: Embed {
            camera: Some(assembled.exif),
            camera_fallback: policy.camera,
            xmp,
            ppi: settings.ppi,
        },
        notice,
    })
}

/// What stands in for the camera's EXIF of `source`, which has none that
/// can be read: the decoder's capture settings in `metadata`, with the
/// dates a RAW container's own records give (a CRW's TimeStamp).
pub fn stand_in(source: &Path, metadata: &crate::camera_data::Metadata) -> exif::CameraExif {
    let mut stand_in = exif::CameraExif::from_libraw(metadata);
    stand_in.exif.extend(crate::raw::capture_date_tags(source));
    stand_in
}

/// `raw` at full resolution, as Develop decodes the photo it opens: the decode
/// cache's copy when it has one.
pub fn decode_full(
    raw: Raw,
    source: &Path,
    demosaic: Demosaic,
    cache: &DecodeCache,
    cancel: Cancel<'_>,
) -> Result<CameraImage> {
    let full = FullSize::with_cache(source, demosaic, cache.clone());
    match full.cached(&raw.metadata) {
        Some(image) => Ok(image),
        None => full.decode(raw, DecodePolicy::Export, cancel),
    }
}

/// The complete AI Denoise source of `photo` (full-size `image`) when its edit renders
/// AI Denoise: from the service's cache, a running job, or computed now. An export never
/// renders the plain image, or a partial result, in its place: without one it fails
/// with the reason.
fn ai_source(
    photo: &Photo,
    image: &Arc<CameraImage>,
    cancel: Cancel<'_>,
) -> Result<Option<crate::develop::AiSource>> {
    if photo.from_editable_preview || !photo.recipe.as_rendered().ai_denoise.renders() {
        return Ok(None);
    }
    let file = photo.file.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "AI Denoise: the photo's file could not be identified when it was opened; open it again"
        )
    })?;
    let ai = photo
        .ai
        .0
        .get(&photo.source, &file, image, cancel)
        .map_err(|e| anyhow::anyhow!("AI Denoise: {e:#}"))?;
    let ai = Arc::try_unwrap(ai).unwrap_or_else(|shared| (*shared).clone());
    crate::develop::AiSource::recover(ai, cancel).map(Some)
}

/// The full-resolution image: the open one, the decode cache's, or a new decode.
fn full_size(
    image: Arc<CameraImage>,
    source: &Path,
    demosaic: Demosaic,
    cache: &DecodeCache,
    cancel: Cancel<'_>,
) -> Result<Arc<CameraImage>> {
    if !image.fast {
        return Ok(image);
    }
    let full = FullSize::with_cache(source, demosaic, cache.clone());
    Ok(Arc::new(match full.cached(&image.metadata) {
        Some(cached) => cached,
        None => full.decode(crate::photo::open(source)?, DecodePolicy::Export, cancel)?,
    }))
}

fn xmp(
    photo: &Photo,
    image: &CameraImage,
    settings: &ExportSettings,
    fields: &super::assemble::XmpFields,
) -> String {
    crate::xmp::write::packet(
        &photo.recipe,
        &image.metadata,
        &crate::xmp::write::Photo {
            creator_tool: crate::build_info::SOFTWARE.into(),
            raw_name: photo
                .source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            captured: fields.captured.clone(),
            created: fields.created.clone(),
            now: crate::time::now_xmp(),
            rating: fields.rating,
            label: fields.label.clone(),
            keywords: fields.keywords.clone(),
            title: fields.title.clone(),
            caption: fields.caption.clone(),
            rights: fields.rights.clone(),
            creators: fields.creators.clone(),
            lens: fields.lens,
            settings: fields.develop,
            format: settings.mime_type().into(),
            crop: Some(crate::develop::rendered_crop(
                &photo.recipe,
                &image.metadata,
            )),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::{CameraExif, Field, tag};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The photo at `source`, its image decoded already, as Develop has it.
    fn photo(source: &Path, cache: &Path) -> Photo {
        let metadata = crate::camera_data::Metadata {
            make: "Canon".into(),
            model: "EOS 300D".into(),
            shutter: 0.01,
            aperture: 4.,
            iso: 200.,
            focal: 35.,
            ..Default::default()
        };
        Photo {
            image: Arc::new(CameraImage {
                width: 4,
                height: 2,
                recovered: Default::default(),
                pixels: vec![[0.2; 3]; 8],
                metadata,
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
                hdr: Default::default(),
            }),
            source: source.to_path_buf(),
            recipe: Recipe::default(),
            values: Default::default(),
            watermark: None,
            demosaic: Demosaic::default(),
            cache: DecodeCache::new(cache.to_path_buf(), u64::MAX),
            ai: crate::ai_denoise::AiService::refusing("no AI Denoise here"),
            file: None,
            from_editable_preview: false,
        }
    }

    /// An AI Denoise service that answers with a fixed pattern, and counts its calls.
    struct Flat(std::sync::atomic::AtomicUsize, f32);
    impl crate::ai_denoise::AiBase for Flat {
        fn get(
            &self,
            _: &Path,
            _: &crate::storage::Identity,
            plain: &Arc<CameraImage>,
            _: Cancel<'_>,
        ) -> Result<Arc<CameraImage>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut image = (**plain).clone();
            image.recovered = Default::default();
            // Not the plain image's pattern: rows instead of columns.
            for (i, p) in image.pixels.iter_mut().enumerate() {
                *p = [0.02 + self.1 * ((i / plain.width as usize) % 7) as f32 / 30.; 3];
            }
            Ok(Arc::new(image))
        }
    }

    fn with_ai(mut photo: Photo, amount: f32) -> Photo {
        photo.recipe.ai_denoise = crate::model::ai_denoise::AiDenoise {
            enabled: true,
            amount,
        };
        photo
    }

    /// Without the AI result, an export with AI Denoise on fails with the reason and
    /// writes nothing; it never renders the plain image in its place.
    #[test]
    fn an_export_with_ai_denoise_and_no_service_fails_closed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("a.dng");
        std::fs::write(&source, b"synthetic")?;
        let mut photo = with_ai(photo(&source, &dir.path().join("cache")), 1.);
        photo.file = crate::storage::Identity::read(&source).ok();
        let target = dir.path().join("out.jpg");
        let error = run(
            photo,
            &ExportSettings::default(),
            &target,
            super::super::Replace::NoClobber,
            Cancel::NEVER,
            |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("no AI Denoise here"), "{error}");
        assert!(!target.exists());
        Ok(())
    }

    /// A photo whose file could not be identified when it was decoded gets no AI result:
    /// nothing proves the service's would be of the same file.
    #[test]
    fn an_export_without_the_decoded_files_identity_fails_closed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("a.dng");
        std::fs::write(&source, b"synthetic")?;
        let flat = Arc::new(Flat(Default::default(), 0.6));
        let mut photo = with_ai(photo(&source, &dir.path().join("cache")), 1.);
        photo.ai = crate::ai_denoise::AiService(flat.clone());
        let error = prepare(&photo, &ExportSettings::default(), Cancel::NEVER, &|_| {})
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("could not be identified"),
            "{error}"
        );
        assert_eq!(flat.0.load(Ordering::SeqCst), 0);
        Ok(())
    }

    /// The service's source is what the export renders, blended by Amount; with AI
    /// Denoise off the service is never asked and the render is the plain one.
    #[test]
    fn an_export_renders_the_services_source_and_off_never_asks() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("a.dng");
        std::fs::write(&source, b"synthetic")?;
        let flat = Arc::new(Flat(Default::default(), 0.6));
        let render = |photo: &Photo| -> Result<Vec<[f32; 3]>> {
            match prepare(photo, &ExportSettings::default(), Cancel::NEVER, &|_| {})?.rendered {
                crate::export::Pixels::Sdr(image) => Ok(image.pixels().to_vec()),
                crate::export::Pixels::Hdr(_) | crate::export::Pixels::GainMap(_) => {
                    anyhow::bail!("an SDR export rendered HDR")
                }
            }
        };
        let mut plain = photo(&source, &dir.path().join("cache"));
        plain.ai = crate::ai_denoise::AiService(flat.clone());
        plain.file = crate::storage::Identity::read(&source).ok();
        // Big enough for the tone stage to keep a pattern.
        let mut image = (*plain.image).clone();
        (image.width, image.height) = (64, 48);
        // A frame of that size and a camera that renders (the engine's fixtures').
        (image.metadata.width, image.metadata.height) = (64, 48);
        image.metadata.matrix = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        image.metadata.wb = [1.; 3];
        image.metadata.daylight_wb = [1.; 3];
        image.pixels = (0..64 * 48)
            .map(|i| [0.02 + (i % 64) as f32 / 300.; 3])
            .collect();
        plain.image = Arc::new(image);
        let off = render(&plain)?;
        // Off at a moved Amount, and on at Amount 0, are the plain render.
        let mut moved = plain.clone();
        moved.recipe.ai_denoise.amount = 0.9;
        assert_eq!(render(&moved)?, off);
        assert_eq!(render(&with_ai(plain.clone(), 0.))?, off);
        assert_eq!(flat.0.load(Ordering::SeqCst), 0);
        let full = render(&with_ai(plain.clone(), 1.))?;
        let half = render(&with_ai(plain, 0.5))?;
        assert_eq!(flat.0.load(Ordering::SeqCst), 2);
        assert_ne!(full, off);
        // Half way is neither (the tone stage is not linear, so not checked as a mean).
        assert!(half != full && half != off);
        Ok(())
    }

    /// An editable preview renders as it is: reduced (`fast`), with no original to
    /// decode or read at `source`, it is never decoded again in its place, the AI
    /// Denoise service is never asked, and the camera's EXIF stands in from its
    /// metadata.
    #[test]
    fn an_editable_preview_renders_as_it_is_without_its_original() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("offline.dng");
        let flat = Arc::new(Flat(Default::default(), 0.6));
        let mut photo = with_ai(photo(&source, &dir.path().join("cache")), 1.);
        photo.ai = crate::ai_denoise::AiService(flat.clone());
        photo.file = None;
        let mut image = (*photo.image).clone();
        image.fast = true;
        photo.image = Arc::new(image);
        // Without the flag, the reduced image would be decoded from the original.
        let error = prepare(&photo, &ExportSettings::default(), Cancel::NEVER, &|_| {})
            .err()
            .expect("no original to decode");
        assert!(!error.to_string().is_empty());
        photo.from_editable_preview = true;
        let exif = exported(photo, dir.path())?;
        assert_eq!(flat.0.load(Ordering::SeqCst), 0);
        let target = dir.path().join("out.jpg");
        assert_eq!(image::image_dimensions(&target)?, (4, 2));
        assert!(
            exif.main.iter().chain(&exif.exif).next().is_some(),
            "the stand-in EXIF is written"
        );
        Ok(())
    }

    /// The EXIF of `photo`'s JPEG export.
    fn exported(photo: Photo, dir: &Path) -> Result<CameraExif> {
        let settings = ExportSettings {
            include: crate::export_settings::Include::AllExceptCameraRaw,
            ..Default::default()
        };
        let target = dir.join("out.jpg");
        let _ = std::fs::remove_file(&target);
        let cancel = AtomicBool::new(false);
        run(
            photo,
            &settings,
            &target,
            super::super::Replace::NoClobber,
            Cancel::new(&cancel),
            |_| {},
        )?;
        crate::exif::read(&target).ok_or_else(|| anyhow::anyhow!("no EXIF in the export"))
    }

    fn text(exif: &CameraExif, tag: u16) -> Option<String> {
        exif.main
            .iter()
            .chain(&exif.exif)
            .chain(&exif.gps)
            .find(|f| f.tag == tag)
            .and_then(Field::text)
    }

    /// A CR3's and an MRW's camera EXIF go with their exports as a CR2's
    /// does: the creator, copyright and description, the dates, the
    /// serial numbers, the exposure program and GPS (#671).
    #[test]
    fn a_cr3s_and_an_mrws_camera_exif_go_with_their_exports() -> Result<()> {
        use super::super::exif::{Container, camera_raw};
        let camera = CameraExif {
            main: vec![
                Field::ascii(tag::IMAGE_DESCRIPTION, "Harbour at dusk"),
                Field::ascii(tag::MAKE, "Canon"),
                Field::ascii(tag::MODEL, "Canon EOS R5m2"),
                Field::ascii(tag::ARTIST, "Ann Example"),
                Field::ascii(tag::COPYRIGHT, "Ann Example 2024"),
            ],
            exif: vec![
                Field::short(0x8822, 1),
                Field::ascii(tag::DATE_TIME_ORIGINAL, "2024:08:20 15:24:34"),
                Field::ascii(tag::DATE_TIME_DIGITIZED, "2024:08:20 15:24:35"),
                Field::ascii(tag::OFFSET_TIME_ORIGINAL, "+02:00"),
                Field::ascii(0xa431, "143302103530"),
                Field::ascii(tag::LENS_MODEL, "RF135mm F1.8 L IS USM"),
                Field::ascii(tag::MAKER_NOTE, "maker note"),
            ],
            gps: vec![
                Field::ascii(tag::GPS_LATITUDE_REF, "N"),
                Field::rational(tag::GPS_LATITUDE, 52, 1),
            ],
        };
        let dir = tempfile::tempdir()?;
        for (container, name) in [(Container::Cr3, "a.CR3"), (Container::Mrw, "a.MRW")] {
            let source = dir.path().join(name);
            std::fs::write(&source, camera_raw(container, camera.clone()))?;
            let out = exported(photo(&source, dir.path()), dir.path())?;
            for (tag, value) in [
                (tag::IMAGE_DESCRIPTION, "Harbour at dusk"),
                (tag::MODEL, "Canon EOS R5m2"),
                (tag::ARTIST, "Ann Example"),
                (tag::COPYRIGHT, "Ann Example 2024"),
                (tag::DATE_TIME_ORIGINAL, "2024:08:20 15:24:34"),
                (tag::DATE_TIME_DIGITIZED, "2024:08:20 15:24:35"),
                (tag::OFFSET_TIME_ORIGINAL, "+02:00"),
                (0xa431, "143302103530"),
                (tag::LENS_MODEL, "RF135mm F1.8 L IS USM"),
                (tag::GPS_LATITUDE_REF, "N"),
            ] {
                assert_eq!(text(&out, tag).as_deref(), Some(value), "{name} {tag:#x}");
            }
            let program = out.exif.iter().find(|f| f.tag == 0x8822);
            assert_eq!(program.and_then(Field::number), Some(1.), "{name}");
            assert_eq!(out.gps.len(), 2, "{name}");
            // The maker note's offsets point into the RAW.
            assert!(out.get(tag::MAKER_NOTE).is_none(), "{name}");
        }
        Ok(())
    }

    /// A CRW has no EXIF: its export carries the decoder's capture settings
    /// and the date of its TimeStamp record (#671).
    #[test]
    fn a_crws_export_is_dated_by_its_time_stamp() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("a.CRW");
        // 2016-12-23 17:05:30 on the camera's clock.
        std::fs::write(&source, super::super::exif::dated_crw(1_482_512_730))?;
        let out = exported(photo(&source, dir.path()), dir.path())?;
        assert_eq!(
            text(&out, tag::DATE_TIME_ORIGINAL).as_deref(),
            Some("2016:12:23 17:05:30")
        );
        assert_eq!(text(&out, tag::MODEL).as_deref(), Some("EOS 300D"));
        let f_number = out.exif.iter().find(|f| f.tag == tag::F_NUMBER);
        assert_eq!(f_number.and_then(Field::number), Some(4.));
        Ok(())
    }

    /// Export reads the full-size image from the cache it is given, which
    /// follows the folder chosen in Preferences, not the default directory.
    #[test]
    fn a_full_size_decode_reads_the_given_cache() -> Result<()> {
        let chart = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let raw = crate::photo::open(&chart)?;
        let cached = CameraImage {
            width: 4,
            height: 3,
            recovered: Default::default(),
            pixels: vec![[0.5; 3]; 12],
            metadata: raw.metadata.clone(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        let _ = cached.recovered.set(Arc::new(cached.clone()));
        cache.store(&DecodeCache::key(&chart, Demosaic::default())?, &cached)?;
        let image = decode_full(raw, &chart, Demosaic::default(), &cache, Cancel::NEVER)?;
        assert_eq!((image.width, image.height), (4, 3));
        Ok(())
    }

    /// A full-size export read from the decode cache measures its own lateral
    /// chromatic aberration (Auto CA), never the one the half-size image on screen
    /// was measured with, as a decode of the file would (#868).
    #[test]
    fn a_cached_full_size_image_keeps_no_half_size_measurement() -> Result<()> {
        let chart = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let metadata = crate::photo::open(&chart)?.metadata;
        let image = |width: u32, height: u32, fast: bool| {
            let image = CameraImage {
                width,
                height,
                recovered: Default::default(),
                pixels: vec![[0.5; 3]; (width * height) as usize],
                metadata: metadata.clone(),
                fast,
                scale_factor: 1.,
                scale_clipped: 0,
                hdr: Default::default(),
            };
            let _ = image.recovered.set(Arc::new(image.clone()));
            image
        };
        cache.store(
            &DecodeCache::key(&chart, Demosaic::default())?,
            &image(4, 3, false),
        )?;
        // The half-size image, measured by a preview render.
        let half = Arc::new(image(2, 2, true));
        let measured = rawmakase_model::optics::Radial::new(vec![0., 1.], vec![1., 1.001]).unwrap();
        let _ = half
            .metadata
            .lateral_ca
            .set(Some([measured.clone(), measured]));
        let full = full_size(half, &chart, Demosaic::default(), &cache, Cancel::NEVER)?;
        assert_eq!((full.width, full.height), (4, 3));
        assert!(full.metadata.lateral_ca.get().is_none());
        // Its recovered image is the same decode, with the same measurement.
        assert!(std::sync::Arc::ptr_eq(
            &full.metadata.lateral_ca,
            &full.recovered.get().unwrap().metadata.lateral_ca
        ));
        Ok(())
    }
}
