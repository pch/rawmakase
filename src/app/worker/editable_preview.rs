//! The app's one seam to an editable preview's file (docs/editable-previews.md),
//! which `rawmakase_export::export::editable_preview` writes and
//! `rawmakase_decode::editable_preview` opens. Develop's loader
//! opens a copy through [`CopyFiles::open`] and Build Editable Previews makes one
//! through [`CopyFiles::build`]; nothing else in the app reads or writes a copy's
//! contents. The catalog only records which file is whose
//! (`catalog::editable_previews`).
//!
//! Tests stand in for the decoder with a [`CopyFiles`] of their own, so the
//! app's rules around a copy (when one opens, what its saves are checked
//! against, when a build is recorded) are tested without a real RAW.
use crate::camera_data::{CameraImage, Demosaic};
use crate::camera_profiles::CameraProfile;
use crate::storage::Identity;
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

/// What one build of a copy reads, captured when it was asked for.
pub(crate) struct BuildInput<'a> {
    /// The original's file.
    pub path: &'a Path,
    /// The original as the build read it before anything else did: what the
    /// copy is of, and what the catalog records it was built from.
    pub identity: &'a Identity,
    pub demosaic: Demosaic,
    /// Where the original's full-size decode is looked up first.
    pub decode_cache: &'a crate::decode_cache::DecodeCache,
}

/// Reads and writes editable previews' files.
pub(crate) trait CopyFiles: Send + Sync {
    /// Opens the copy at `path` as its photo. Any error means the copy is out
    /// of date: an unknown record version, a damaged or missing file.
    fn open(&self, path: &Path, cancel: Cancel<'_>) -> anyhow::Result<OpenedCopy>;
    /// Builds the copy of `input`'s original and writes it to `dest`
    /// atomically, replacing a file there, or fails having written nothing.
    fn build(&self, input: &BuildInput<'_>, dest: &Path, cancel: Cancel<'_>) -> anyhow::Result<()>;
}

/// The copy files the app uses, shared by the loader and the build worker.
pub(crate) type Copies = Arc<dyn CopyFiles>;

/// The copy files export writes and the decoder reads.
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
    /// The original decoded in full as Develop decodes it (the decode cache's
    /// copy of this version when it has one, never stored from here, as an
    /// export's), with the default Camera Look a camera without colour data
    /// starts from, written as its copy.
    fn build(&self, input: &BuildInput<'_>, dest: &Path, cancel: Cancel<'_>) -> anyhow::Result<()> {
        use anyhow::Context as _;
        let raw = crate::photo::open(input.path)
            .with_context(|| format!("{} can't be read", input.path.display()))?;
        let (profiles, _) = crate::camera_profiles::installed(&raw.metadata);
        let listed = profiles.len();
        let profiles =
            crate::camera_look::with_default(input.path, &raw.metadata, profiles, cancel);
        // `with_default` adds the fitted look last, and only for such a camera.
        let look = (profiles.len() > listed)
            .then(|| profiles.last().cloned())
            .flatten();
        let full = crate::decode::FullSize::of_version(
            Some(input.identity),
            input.demosaic,
            input.decode_cache.clone(),
        );
        let image = match full.cached(&raw.metadata) {
            Some(image) => image,
            None => full.decode(raw, crate::decode::DecodePolicy::Export, cancel)?,
        };
        crate::export::editable_preview::write(dest, &image, look.as_deref(), cancel)
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

    /// What the stand-in does as it builds, before it writes: fails, panics or
    /// waits, as a test needs.
    pub(crate) type Hook = Arc<dyn Fn(&Path) -> anyhow::Result<()> + Send + Sync>;

    #[derive(Default)]
    pub(crate) struct StandIn {
        before_build: Option<Hook>,
    }
    impl StandIn {
        pub(crate) fn copies() -> Copies {
            Arc::new(Self::default())
        }
        /// Copies that run `hook` with the original's path before each build
        /// writes.
        pub(crate) fn building(
            hook: impl Fn(&Path) -> anyhow::Result<()> + Send + Sync + 'static,
        ) -> Copies {
            Arc::new(Self {
                before_build: Some(Arc::new(hook)),
            })
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
        fn build(
            &self,
            input: &BuildInput<'_>,
            dest: &Path,
            _cancel: Cancel<'_>,
        ) -> anyhow::Result<()> {
            if let Some(hook) = &self.before_build {
                hook(input.path)?;
            }
            write(dest, input.identity)
        }
    }
}

#[cfg(test)]
mod decoder_tests {
    use super::*;

    /// The app's decoder builds a copy of a real file and opens it again as the
    /// photo it was built from: a reduced image of the original's size.
    #[test]
    fn the_decoder_builds_a_copy_it_opens_as_its_photo() {
        let original =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/fujifilm-x-t5-d65.dng");
        let dir = tempfile::tempdir().unwrap();
        let cache = crate::decode_cache::DecodeCache::new(dir.path().join("cache"), u64::MAX);
        let identity = Identity::read(&original).unwrap();
        let dest = dir.path().join("copy.dng");
        let input = BuildInput {
            path: &original,
            identity: &identity,
            demosaic: Demosaic::default(),
            decode_cache: &cache,
        };
        decoder().build(&input, &dest, Cancel::NEVER).unwrap();
        let opened = decoder().open(&dest, Cancel::NEVER).unwrap();
        assert!(opened.image.fast, "a reduced decode");
        let full = crate::photo::open(&original).unwrap().metadata;
        assert_eq!(
            (opened.image.metadata.width, opened.image.metadata.height),
            (full.width, full.height),
            "of a photo of the original's size"
        );
        assert!(opened.image.recovered.get().is_some(), "recovered once");
        // Built again over it: replaced, still opening.
        decoder().build(&input, &dest, Cancel::NEVER).unwrap();
        assert!(decoder().open(&dest, Cancel::NEVER).is_ok());
    }
}
