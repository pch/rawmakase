//! The app's one seam to an editable preview's file (docs/editable-previews.md),
//! which `rawmakase_export::export::editable_preview` writes and
//! `rawmakase_decode::editable_preview` opens. Develop's loader
//! opens a copy through [`CopyFiles::open`]; nothing else in the app reads a
//! copy's contents. The catalog only records which file is whose
//! (`catalog::editable_previews`).
//!
//! Tests stand in for the decoder with a [`CopyFiles`] of their own, so the
//! app's rules around a copy (when one opens, what its saves are checked
//! against) are tested without a real RAW.
use crate::camera_data::CameraImage;
use crate::camera_profiles::CameraProfile;
use rawmakase_cancel::Cancel;
use std::path::Path;
use std::sync::Arc;

/// An editable preview opened as its photo, as
/// `rawmakase_decode::editable_preview::EditablePreview` gives it.
pub(crate) struct OpenedCopy {
    /// A reduced image ([`CameraImage::fast`]) of a photo of the original's
    /// size, with the original's metadata and its highlights recovered already.
    pub image: CameraImage,
    /// The photo's fitted Camera Look, for a camera without colour data: its
    /// fit needs the original, so offline it comes only from the copy.
    pub default_look: Option<CameraProfile>,
}

/// Reads editable previews' files.
pub(crate) trait CopyFiles: Send + Sync {
    /// Opens the copy at `path` as its photo. Any error means the copy is out
    /// of date: an unknown record version, a damaged or missing file.
    fn open(&self, path: &Path, cancel: Cancel<'_>) -> anyhow::Result<OpenedCopy>;
}

/// The copy files the app uses.
pub(crate) type Copies = Arc<dyn CopyFiles>;

/// The copy files the decoder reads.
pub(crate) struct Decoder;

impl CopyFiles for Decoder {
    fn open(&self, path: &Path, cancel: Cancel<'_>) -> anyhow::Result<OpenedCopy> {
        let crate::editable_preview::EditablePreview {
            image,
            default_look,
        } = crate::editable_preview::open(path, cancel)?;
        Ok(OpenedCopy {
            image,
            default_look,
        })
    }
}

/// The copy files the app uses by default.
pub(crate) fn decoder() -> Copies {
    Arc::new(Decoder)
}

/// A stand-in for the decoder in tests: a copy's file holds the identity it
/// was built from as JSON, and opens as a small image of the photo.
#[cfg(test)]
pub(crate) mod stand_in {
    use super::*;
    use crate::storage::Identity;

    pub(crate) struct StandIn;
    impl StandIn {
        pub(crate) fn copies() -> Copies {
            Arc::new(Self)
        }
    }
    /// Writes the stand-in's copy of a photo whose original was `original`: the
    /// identity, as JSON, so a test can tell which file a copy was built from.
    pub(crate) fn write(dest: &Path, original: &Identity) -> anyhow::Result<()> {
        if let Some(folder) = dest.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(dest, serde_json::to_vec(original)?)?;
        Ok(())
    }
    impl CopyFiles for StandIn {
        fn open(&self, path: &Path, _cancel: Cancel<'_>) -> anyhow::Result<OpenedCopy> {
            // Anything else, as a damaged copy, fails to open.
            let _: Identity = serde_json::from_slice(&std::fs::read(path)?)?;
            let (width, height) = (32, 24);
            let metadata = crate::camera_data::Metadata {
                width: width * 4,
                height: height * 4,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            };
            Ok(OpenedCopy {
                image: CameraImage {
                    recovered: Default::default(),
                    width,
                    height,
                    pixels: vec![[0.2, 0.3, 0.4]; (width * height) as usize].into(),
                    metadata,
                    fast: true,
                    scale_factor: 1.,
                    scale_clipped: 0,
                    hdr: Default::default(),
                },
                default_look: None,
            })
        }
    }
}

#[cfg(test)]
mod decoder_tests {
    use super::*;

    /// The app's decoder opens a copy export wrote of a real file as the photo
    /// it was written from: a reduced image of the original's size.
    #[test]
    fn the_decoder_opens_a_copy_as_its_photo() {
        let original =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/fujifilm-x-t5-d65.dng");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("copy.dng");
        let full = crate::photo::open(&original)
            .unwrap()
            .develop(
                crate::camera_data::Decode::Full(Default::default()),
                Cancel::NEVER,
            )
            .unwrap();
        crate::export::editable_preview::write(&dest, &full, None, Cancel::NEVER).unwrap();
        let opened = decoder().open(&dest, Cancel::NEVER).unwrap();
        assert!(opened.image.fast, "a reduced decode");
        assert_eq!(
            (opened.image.metadata.width, opened.image.metadata.height),
            (full.metadata.width, full.metadata.height),
            "of a photo of the original's size"
        );
        assert!(opened.image.recovered.get().is_some(), "recovered once");
    }
}
