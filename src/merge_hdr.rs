//! Merge to HDR as the command line and the Library publish it, after the
//! merge itself (`rawmakase_hdr_merge::merge`): the merged DNG's starting edit,
//! the XMP and preview it carries, and the file put in place beside the
//! active photo. Never reaches `app` (`scripts/deps-closures.txt`): what the
//! desktop session keeps, the raw defaults, is passed in.
//!
//! The starting edit is Lightroom Classic 15.6's (docs/hdr-merge.md): the
//! merge's own raw defaults, with the active photo's white balance, profile
//! (and its look) and lens profile corrections, the crop that leaves out what
//! Auto Align left uncovered, and High Dynamic Range off.
use crate::camera_data::{Decode, Metadata};
use crate::camera_profiles::CameraProfile;
use crate::edits::EditRecord;
use crate::model::image_frame::ImageFrame;
use crate::model::recipe::Recipe;
use crate::model::settings_groups::{
    self, GroupInclusion, GroupSelection, SettingGroup, Source, Target,
};
use crate::raw_defaults::DevelopDefaults;
use anyhow::{Context, Result};
use rawmakase_cancel::Cancel;
use rawmakase_hdr_merge::{Destination, Error, Extras, Merged, Preview, write_dng};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The long edge of the preview the DNG carries, as Lightroom's merges carry
/// one.
pub const PREVIEW_EDGE: u32 = 1024;

/// The JPEG quality of that preview.
const PREVIEW_QUALITY: u8 = 90;

/// The photo a merge starts from, Lightroom's active photo: its edit as it
/// develops, and its camera.
#[derive(Clone, Copy)]
pub struct Active<'a> {
    pub recipe: &'a Recipe,
    pub metadata: &'a Metadata,
}

/// A merge put in place.
#[derive(Debug)]
pub struct Published {
    pub path: PathBuf,
    /// Its starting edit, which its XMP holds and the catalog stores.
    pub recipe: Recipe,
    /// What of the active photo's settings could not be carried over.
    pub notes: Vec<String>,
}

/// The settings of the active photo a merge starts with.
fn carried() -> GroupSelection {
    let mut groups = GroupSelection::none();
    for group in [
        SettingGroup::WhiteBalance,
        SettingGroup::TreatmentAndProfile,
        SettingGroup::LensProfileCorrections,
    ] {
        groups.set(group, GroupInclusion::Included);
    }
    groups
}

/// The merge's starting edit: `defaults`, its raw defaults (`m` its camera,
/// `profiles` the camera profiles for it), with `active`'s white balance,
/// profile and lens profile corrections, `crop` (the merge's) and High
/// Dynamic Range off; and what could not be carried over. `crop` is of the
/// merge's unrotated frame (as `Merged::crop`); the edit's is turned with the
/// photo.
pub fn starting_edit(
    defaults: &Recipe,
    active: Active<'_>,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
    crop: Option<[f32; 4]>,
) -> (Recipe, Vec<String>) {
    let transferred = settings_groups::transfer(
        Source {
            recipe: active.recipe,
            metadata: active.metadata,
        },
        defaults,
        &carried(),
        Target {
            metadata: m,
            profiles,
        },
    );
    let mut recipe = transferred.recipe;
    if let Some(crop) = crop {
        recipe.crop = ImageFrame::for_metadata(m).rect_from_unrotated(crop);
    }
    // Lightroom's merges start with HDR off, whatever the preference for new
    // photos says, and without a limit.
    recipe.hdr = Default::default();
    (recipe, transferred.notes)
}

/// Puts `merged` in place at `destination` with its starting edit, its XMP
/// and its preview: the edit worked out from the merge as it opens (its raw
/// defaults, `raw_defaults` for its camera) and `active`. Fails with
/// [`Error::Cancelled`] inside the error when `cancel` is set before the file
/// is in place, which then isn't published.
pub fn publish(
    merged: &Merged,
    active: Active<'_>,
    raw_defaults: &DevelopDefaults,
    destination: &Destination,
    software: &str,
    cancel: Cancel<'_>,
) -> Result<Published> {
    let folder = destination.folder();
    let failed = |e: &dyn std::fmt::Display| Error::Write {
        folder: folder.to_path_buf(),
        why: e.to_string(),
    };
    // The merge without its XMP and preview, opened as the photo it will be:
    // its camera, its raw defaults and the preview's render. Staged beside
    // where it goes, under a name no photo has, and removed when dropped.
    let provisional = crate::storage::staging_file(folder).map_err(|e| failed(&e))?;
    {
        let mut out = std::io::BufWriter::new(provisional.as_file());
        merged
            .dng
            .with(&Extras::default())?
            .write_to(&mut out)
            .and_then(|()| out.flush())
            .map_err(|e| failed(&e))?;
    }
    let path = provisional.path();
    let raw = crate::photo::open(path).context("The merged DNG can't be read")?;
    let m = raw.metadata.clone();
    let profiles =
        crate::camera_look::with_default(path, &m, crate::camera_profiles::installed(&m).0, cancel);
    let defaults =
        crate::edits::resolve(&EditRecord::default(), path, &m, &profiles, raw_defaults)?.recipe;
    let (recipe, notes) = starting_edit(&defaults, active, &m, &profiles, merged.crop);
    recipe.validate()?;
    let captured = crate::photo::camera_exif(path).and_then(|e| e.captured());
    let preview = preview(raw, &recipe, cancel)?;
    drop(provisional);
    let rendered = crate::time::now_xmp();
    let reference = &merged.inputs[merged.frames[merged.reference].input].path;
    for path in destination.paths() {
        if path.exists() {
            continue;
        }
        let xmp = crate::xmp::write::merged_dng_packet(
            &recipe,
            &m,
            &crate::xmp::write::MergedDng {
                creator_tool: software.into(),
                name: file_name(&path),
                reference: file_name(reference),
                captured: captured.clone(),
                now: rendered.clone(),
            },
        );
        let extras = Extras {
            xmp: Some(&xmp),
            preview: preview.as_ref().map(|(jpeg, width, height)| Preview {
                jpeg,
                width: *width,
                height: *height,
                application: ("RAWmakase", env!("CARGO_PKG_VERSION")),
                rendered: &rendered,
            }),
        };
        // A name taken since it was found free goes to the next.
        match write_dng(merged, &extras, &Destination::Exactly(path), cancel) {
            Err(Error::Exists(_)) if matches!(destination, Destination::Beside { .. }) => {}
            written => {
                return Ok(Published {
                    path: written?,
                    recipe,
                    notes,
                });
            }
        }
    }
    let last = destination.paths().last().unwrap_or_default();
    Err(Error::Exists(last).into())
}

/// The merge as `recipe` renders it, within [`PREVIEW_EDGE`], unrotated as
/// a DNG's previews are kept: a JPEG and its size. `None` when it can't be
/// rendered, which leaves the DNG without one; a cancel is an error.
fn preview(
    raw: crate::raw::Raw,
    recipe: &Recipe,
    cancel: Cancel<'_>,
) -> Result<Option<(Vec<u8>, u32, u32)>> {
    let rendered = raw.develop(Decode::Half, cancel).and_then(|mut image| {
        // Readers turn a DNG's preview by its Orientation, as the image: it
        // is rendered unturned, the crop with it.
        let recipe = Recipe {
            crop: ImageFrame::for_metadata(&image.metadata).rect_to_unrotated(recipe.crop),
            ..recipe.clone()
        };
        image.metadata.flip = 0;
        crate::develop::render_fitted(&image, &recipe, PREVIEW_EDGE, cancel)
    });
    if cancel.is_cancelled() {
        return Err(Error::Cancelled.into());
    }
    let Ok(rendered) = rendered else {
        return Ok(None);
    };
    let mut jpeg = Vec::new();
    let encoded = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, PREVIEW_QUALITY)
        .encode(
            &rendered.rgb8(),
            rendered.width,
            rendered.height,
            image::ExtendedColorType::Rgb8,
        );
    Ok(encoded
        .ok()
        .map(|()| (jpeg, rendered.width, rendered.height)))
}

/// The active photo as it develops: its edit resolved from `record` (its
/// catalog edit, or none) with `raw_defaults`, its camera, and what resolving
/// it warned of.
pub struct ActiveEdit {
    pub recipe: Recipe,
    pub metadata: Metadata,
    pub warnings: Vec<String>,
}

impl ActiveEdit {
    /// The active photo at `path` with edit `record`.
    pub fn resolve(
        record: &EditRecord,
        path: &Path,
        raw_defaults: &DevelopDefaults,
        cancel: Cancel<'_>,
    ) -> Result<Self> {
        let metadata = crate::photo::header_metadata(path)
            .or_else(|_| crate::photo::open(path).map(|raw| raw.metadata))?;
        let profiles = crate::camera_look::with_default(
            path,
            &metadata,
            crate::camera_profiles::installed(&metadata).0,
            cancel,
        );
        let resolved = crate::edits::resolve(record, path, &metadata, &profiles, raw_defaults)?;
        Ok(Self {
            recipe: resolved.recipe,
            metadata,
            warnings: resolved.warnings,
        })
    }

    pub fn active(&self) -> Active<'_> {
        Active {
            recipe: &self.recipe,
            metadata: &self.metadata,
        }
    }
}

/// Adds `published` to `catalog`, with its starting edit as its catalog edit,
/// in one transaction; `defaults` are what a new photo starts with. False when
/// the catalog had it already, and keeps its edit there. The errors say where
/// the DNG was saved.
pub fn import(
    catalog: &mut crate::catalog::Catalog,
    published: &Published,
    defaults: &crate::catalog::NewPhotoDefaults,
) -> Result<bool> {
    let path = &published.path;
    let saved = || {
        format!(
            "The merge was saved at {}, but couldn't be added to the catalog",
            path.display()
        )
    };
    let added = catalog
        .import_files_with(
            crate::storage::parent_dir(path),
            std::slice::from_ref(path),
            &[(path.clone(), published.recipe.clone())],
            defaults,
            || true,
        )
        .with_context(saved)?
        .context("the import was withdrawn")
        .with_context(saved)?;
    if let Some((_, why)) = added.refused.first() {
        use crate::catalog::FileRefusal;
        return Err(anyhow::anyhow!(match why {
            FileRefusal::NotFound => "it is no longer there",
            FileRefusal::NotAPhoto => "the catalog doesn't read it as a photo",
            FileRefusal::OutsideFolder => "it isn't in its folder",
            FileRefusal::LinkedElsewhere =>
                "its folder in the catalog is linked to another place on this computer",
            FileRefusal::Ambiguous => "more than one folder of the catalog is where it is",
        })
        .context(saved()));
    }
    Ok(!added.added.is_empty())
}

/// What a merge is doing at `stage`, of a bracket of `frames`; `None` for
/// the stages too short to say.
pub fn stage_text(stage: rawmakase_hdr_merge::Stage, frames: usize) -> Option<String> {
    use rawmakase_hdr_merge::Stage;
    Some(match stage {
        Stage::Check => "Checking the bracket".to_string(),
        Stage::Decode(n) => format!("Decoding frame {} of {frames}", n + 1),
        Stage::Align(n) => format!("Aligning frame {} of {frames}", n + 1),
        Stage::Deghost(n) => format!("Deghosting frame {} of {frames}", n + 1),
        Stage::Measure(_) | Stage::Merge(_) => return None,
        Stage::Encode => "Writing the DNG".to_string(),
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rawmakase_hdr_merge::{Calibration, DngFacts, FrameInput, LinearImage, MergedFrame};
    use rawmakase_interop::exif::{CameraExif, Field, tag};

    const W: u32 = 480;
    const H: u32 = 320;
    /// A camera's D65 colour matrix (XYZ to camera).
    const MATRIX: [f64; 9] = [
        0.7888, -0.1902, -0.1011, -0.8106, 1.643, 0.2099, -0.1688, 0.1961, 0.6278,
    ];

    /// A merge of three source files in `dir` (IMG_1 to IMG_3, the second
    /// the reference), turned a quarter (orientation 6), its crop `crop`.
    pub(crate) fn merged(dir: &Path, crop: Option<[f32; 4]>) -> Merged {
        let inputs: Vec<FrameInput> = (1..=3)
            .map(|n| {
                let path = dir.join(format!("IMG_{n}.CR3"));
                std::fs::write(&path, format!("source {n}")).unwrap();
                FrameInput::read(&path).unwrap()
            })
            .collect();
        let pixels: Vec<[f32; 3]> = (0..W * H)
            .map(|i| {
                let v = 16384. * 2f32.powf(-12. * (i % W) as f32 / W as f32);
                [v * 0.5, v, v * 0.7]
            })
            .collect();
        let facts = DngFacts {
            make: "Canon".into(),
            model: "Canon EOS 850D".into(),
            unique_camera_model: "Canon EOS 850D".into(),
            software: "RAWmakase test".into(),
            white_level: 32768,
            baseline_exposure: 1.,
            calibrations: vec![Calibration {
                illuminant: 21,
                color_matrix: MATRIX,
                forward_matrix: None,
                camera_calibration: None,
            }],
            calibration_signature: None,
            analog_balance: None,
            as_shot_neutral: [0.5, 1., 0.6],
            orientation: 6,
            default_crop: None,
            exif: CameraExif {
                main: Vec::new(),
                exif: vec![Field::ascii(tag::DATE_TIME_ORIGINAL, "2023:09:13 07:07:46")],
                gps: Vec::new(),
            },
            maker_note: None,
        };
        let image = LinearImage {
            width: W,
            height: H,
            pixels: &pixels,
            coverage: None,
        };
        let frame = |input| MergedFrame {
            input,
            nominal_ev: 0.,
            ev: 0.,
            measured: true,
            alignment: None,
            deghosted: 0.,
        };
        Merged {
            dng: rawmakase_hdr_merge::encoded(&image, &facts, Cancel::NEVER).unwrap(),
            crop,
            inputs,
            frames: vec![frame(0), frame(1), frame(2)],
            reference: 1,
            reference_moved: false,
            baseline_exposure: 1.,
        }
    }

    /// The active photo's edit: a white balance, lens corrections and an
    /// exposure, of a camera like the merge's.
    pub(crate) fn active() -> (Recipe, Metadata) {
        let m = Metadata {
            make: "Canon".into(),
            model: "EOS 850D".into(),
            cam_xyz: std::array::from_fn(|r| std::array::from_fn(|c| MATRIX[r * 3 + c] as f32)),
            ..Default::default()
        };
        let mut recipe = Recipe {
            temperature: 4000.,
            tint: 10.,
            lens_profile: true,
            exposure: 1.5,
            contrast: 30.,
            ..Default::default()
        };
        recipe.update_wb(&m);
        (recipe, m)
    }

    fn publish_in(dir: &Path, merged: &Merged, cancel: Cancel<'_>) -> Result<Published> {
        let (recipe, m) = active();
        publish(
            merged,
            Active {
                recipe: &recipe,
                metadata: &m,
            },
            &DevelopDefaults::default(),
            &Destination::beside(&dir.join("IMG_1.CR3")),
            "RAWmakase test",
            cancel,
        )
    }

    /// Names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_merge_is_published_with_its_starting_edit_xmp_and_preview() {
        let dir = tempfile::tempdir().unwrap();
        let crop = [0.01, 0.02, 0.99, 0.98];
        let merged = merged(dir.path(), Some(crop));
        let published = publish_in(dir.path(), &merged, Cancel::NEVER).unwrap();
        assert_eq!(published.path, dir.path().join("IMG_1-HDR.dng"));
        let r = &published.recipe;
        // The active photo's white balance and lens corrections, not its tone.
        assert_eq!((r.temperature, r.tint), (4000., 10.));
        assert!(r.lens_profile);
        assert_eq!((r.exposure, r.contrast), (0., 0.));
        assert!(r.hdr.is_default());

        let bytes = std::fs::read(&published.path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for attribute in [
            r#"aux:IsMergedHDR="True""#,
            r#"xmpMM:PreservedFileName="IMG_2.CR3""#,
            r#"crs:RawFileName="IMG_1-HDR.dng""#,
            r#"crs:Temperature="4000""#,
            r#"crs:HDREditMode="0""#,
            r#"crs:AlreadyApplied="False""#,
            r#"xmp:CreatorTool="RAWmakase test""#,
        ] {
            assert!(text.contains(attribute), "{attribute}");
        }
        // The preview, as the starting edit crops it and unrotated as the
        // image is kept.
        let mut raw = crate::photo::open(&published.path).unwrap();
        let preview = image::load_from_memory(&raw.thumbnail().unwrap()).unwrap();
        assert_eq!((preview.width(), preview.height()), (470, 307));
        assert_eq!(raw.metadata.flip, 6);
        // Nothing staged is left behind; a second merge takes the next name.
        assert_eq!(
            names(dir.path()),
            ["IMG_1-HDR.dng", "IMG_1.CR3", "IMG_2.CR3", "IMG_3.CR3"]
        );
        let again = publish_in(dir.path(), &merged, Cancel::NEVER).unwrap();
        assert_eq!(again.path, dir.path().join("IMG_1-HDR-2.dng"));
        let text = String::from_utf8_lossy(&std::fs::read(&again.path).unwrap()).into_owned();
        assert!(text.contains(r#"crs:RawFileName="IMG_1-HDR-2.dng""#));
    }

    /// The merge's crop is of its unrotated frame, as its coverage is; the
    /// edit holds it turned with the photo, and Lightroom's XMP unrotated.
    #[test]
    fn the_crop_turns_with_the_photo() {
        let dir = tempfile::tempdir().unwrap();
        let unrotated = [0.05, 0.02, 0.99, 0.98];
        let merged = merged(dir.path(), Some(unrotated));
        let published = publish_in(dir.path(), &merged, Cancel::NEVER).unwrap();
        let raw = crate::photo::open(&published.path).unwrap();
        let frame = crate::model::image_frame::ImageFrame::for_metadata(&raw.metadata);
        assert_eq!(frame.turns % 2, 1, "turned a quarter");
        let crop = published.recipe.crop;
        let want = frame.rect_from_unrotated(unrotated);
        assert!(
            crop.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
            "{crop:?} for {want:?}"
        );
        let text = String::from_utf8_lossy(&std::fs::read(&published.path).unwrap()).into_owned();
        assert!(text.contains(r#"crs:CropLeft="0.050000""#), "{text}");
        assert!(text.contains(r#"crs:CropTop="0.020000""#));
        // The preview, unrotated, shows the same part of the image.
        let mut raw = raw;
        let preview = image::load_from_memory(&raw.thumbnail().unwrap()).unwrap();
        assert_eq!((preview.width(), preview.height()), (451, 307));
    }

    #[test]
    fn a_cancelled_or_changed_merge_publishes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let merged = merged(dir.path(), None);
        let set = std::sync::atomic::AtomicBool::new(true);
        let error = publish_in(dir.path(), &merged, Cancel::new(&set)).unwrap_err();
        assert_eq!(error.downcast_ref::<Error>(), Some(&Error::Cancelled));
        assert_eq!(names(dir.path()), ["IMG_1.CR3", "IMG_2.CR3", "IMG_3.CR3"]);
        // A source changed since it was merged.
        std::fs::write(dir.path().join("IMG_3.CR3"), "changed").unwrap();
        let error = publish_in(dir.path(), &merged, Cancel::NEVER).unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error>(),
            Some(&Error::SourceChanged("IMG_3.CR3".into()))
        );
        assert_eq!(names(dir.path()), ["IMG_1.CR3", "IMG_2.CR3", "IMG_3.CR3"]);
    }
}
