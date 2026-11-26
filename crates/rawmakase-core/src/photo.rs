//! A photo: a file opened for developing, what the library knows of it
//! before its pixels are decoded, and its decode.
use crate::{Context, Error, Image, Result};
use rawmakase_cancel::Cancel;
use rawmakase_decode::decode::FullSize;
use rawmakase_decode::photo::Opened;
use rawmakase_engine::develop::quality;
use rawmakase_interop::edits;
use rawmakase_model::camera_data::{self, CameraImage, Demosaic, Metadata};
use rawmakase_model::camera_profiles::CameraProfile;
use rawmakase_model::model::recipe::Recipe;
use rawmakase_model::storage::Identity;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// How much of a photo a decode makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decode {
    /// A half-size draft, each pixel a 2 × 2 block of the sensor's mosaic: a
    /// first image to show while the full one decodes. A photo that is not a
    /// mosaic (a JPEG, a small RAW) decodes at full size either way.
    Half,
    /// Every pixel, with highlights recovered, from the context's cache when
    /// it holds this file as it is now.
    Full,
}

/// A photo opened for developing: its file's metadata and the camera
/// profiles that fit it. Made by [`Context::open`]. `Send` and `Sync`.
///
/// It keeps no RAW file's bytes: a [`Photo::decode`] reads the file again,
/// and refuses it as [`ErrorKind::Changed`](crate::ErrorKind::Changed) when it
/// is no longer the file that was opened.
pub struct Photo {
    ctx: Context,
    path: PathBuf,
    /// The decoder's facts, which edits are made with.
    metadata: Metadata,
    info: crate::Metadata,
    profiles: Vec<Arc<CameraProfile>>,
    shown: Vec<crate::Profile>,
    profile_errors: Vec<String>,
    /// The file as it was opened; `None` when its identity could not be read,
    /// and then every decode is of whatever is there.
    identity: Option<Identity>,
    /// A rendered file (JPEG, PNG, TIFF…), which opening decodes.
    rendered: Option<Arc<CameraImage>>,
    /// A RAW's last full decode by [`Photo::decode`], while an [`Image`] of
    /// it is held: the next full decode through the same context is that
    /// one.
    full: Mutex<Weak<CameraImage>>,
}

impl Context {
    /// Opens the photo at `path`: a RAW by its extension, anything else as a
    /// rendered file (JPEG, PNG, TIFF, WebP, AVIF, HEIF, JPEG XL).
    ///
    /// A RAW's header and metadata are read, and its pixels are left for
    /// [`Photo::decode`]. A rendered file is decoded here, which is why opening
    /// takes a `cancel`. A photo without color data is fitted its Camera Look
    /// here, from the context's cache when it has one, as the desktop does.
    ///
    /// Errors: [`ErrorKind::Missing`](crate::ErrorKind::Missing) when there is
    /// no file, [`ErrorKind::Unsupported`](crate::ErrorKind::Unsupported) for a
    /// file RAWmakase doesn't develop (a CMYK JPEG, a RAW its decoders don't
    /// read), and [`ErrorKind::Cancelled`](crate::ErrorKind::Cancelled).
    pub fn open(&self, path: impl AsRef<Path>, cancel: Cancel<'_>) -> Result<Photo> {
        let path = path.as_ref();
        cancelled(cancel, "Opening cancelled")?;
        let identity = Identity::read(path).ok();
        let opened =
            rawmakase_decode::photo::open_source(path, self.lenses(), cancel).map_err(Error::of)?;
        let metadata = opened.metadata().clone();
        let listed = self.profiles().listed(&metadata);
        let profiles = rawmakase_decode::camera_look::with_default(
            path,
            self.lenses(),
            &self.fits(),
            &metadata,
            listed.profiles,
            cancel,
        );
        // A Camera Look fit stopped short leaves the photo without it, which
        // would change where it starts: the open is cancelled, not made.
        cancelled(cancel, "Opening cancelled")?;
        let photo = Photo {
            ctx: self.clone(),
            path: path.to_path_buf(),
            info: crate::Metadata::of(&metadata),
            metadata,
            shown: profiles.iter().cloned().map(crate::Profile).collect(),
            profiles,
            profile_errors: listed.errors,
            identity,
            rendered: match opened {
                // Dropped: a decode reads the file again.
                Opened::Raw(_) => None,
                Opened::Rendered(image) => Some(Arc::new(image)),
            },
            full: Mutex::new(Weak::new()),
        };
        // The file read is the one whose identity was taken.
        photo.still_the_file_opened()?;
        Ok(photo)
    }
}

fn cancelled(cancel: Cancel<'_>, message: &'static str) -> Result<()> {
    cancel.check(message).map_err(|c| Error::of(c.into()))
}

impl Photo {
    /// The file this photo was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file says of itself: camera, lens, exposure and size.
    pub fn metadata(&self) -> &crate::Metadata {
        &self.info
    }

    /// The camera profiles that fit this photo, sorted by name.
    pub fn profiles(&self) -> &[crate::Profile] {
        &self.shown
    }

    /// The decoder's facts, which edits are made with.
    pub(crate) fn facts(&self) -> &Metadata {
        &self.metadata
    }

    pub(crate) fn camera_profiles(&self) -> &[Arc<CameraProfile>] {
        &self.profiles
    }

    /// The installed profile files that could not be read, one message each.
    pub fn profile_errors(&self) -> &[String] {
        &self.profile_errors
    }

    pub(crate) fn context(&self) -> &Context {
        &self.ctx
    }

    #[cfg_attr(not(feature = "export"), allow(dead_code))]
    pub(crate) fn identity(&self) -> Option<&Identity> {
        self.identity.as_ref()
    }

    /// The settings this photo starts from when it has no edit: the context's
    /// raw defaults ([`ContextBuilder::raw_defaults`](crate::ContextBuilder::raw_defaults)),
    /// with the profiles that fit, as the desktop opens a new photo.
    ///
    /// [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) when they don't make a
    /// valid recipe.
    pub fn starting_recipe(&self) -> Result<Recipe> {
        let resolved = edits::Start::Defaults(self.ctx.defaults().clone())
            .resolve(&self.metadata, &self.profiles)
            .map_err(Error::of)?;
        resolved
            .recipe
            .validate()
            .map_err(|e| Error::of(rawmakase_model::error::Invalid::from_error(e).into()))?;
        Ok(resolved.recipe)
    }

    /// Decodes the photo's pixels. A long operation: seconds for a large RAW
    /// that is not in the cache.
    ///
    /// A full decode is kept in the context's cache, when it has one, and comes
    /// from there next time. While an [`Image`] of the last full decode is
    /// held, a full decode, and [`Context::export`](crate::Context), use it
    /// and read nothing; otherwise each decode reads the file again. Each
    /// checks that the file is still the one opened.
    ///
    /// Errors: [`ErrorKind::Changed`](crate::ErrorKind::Changed) when the file
    /// is no longer the one opened, [`ErrorKind::Missing`](crate::ErrorKind::Missing)
    /// when it is gone, and [`ErrorKind::Cancelled`](crate::ErrorKind::Cancelled).
    pub fn decode(&self, decode: Decode, cancel: Cancel<'_>) -> Result<Image> {
        self.decode_in(&self.ctx, decode, true, cancel)
    }

    /// [`Photo::decode`] with `ctx`'s lens profiles and cache, keeping a
    /// full decode there only when `keep`: an export reads the cache and
    /// leaves it as it was, as the desktop's does.
    pub(crate) fn decode_in(
        &self,
        ctx: &Context,
        decode: Decode,
        keep: bool,
        cancel: Cancel<'_>,
    ) -> Result<Image> {
        cancelled(cancel, "Decoding cancelled")?;
        self.still_the_file_opened()?;
        let held = || {
            let full = self.full.lock().unwrap_or_else(PoisonError::into_inner);
            // Decoded with this context's lens profiles, as asked.
            full.upgrade().filter(|_| ctx.is(&self.ctx))
        };
        let image = match (&self.rendered, decode) {
            (Some(image), _) => image.clone(),
            (None, Decode::Full) if let Some(image) = held() => image,
            (None, _) => {
                let image = Arc::new(self.decode_raw(ctx, decode, keep, cancel)?);
                // Only a decode the cache has kept: one an export made isn't,
                // and a decode that used it would leave the cache without it.
                if decode == Decode::Full && keep && ctx.is(&self.ctx) {
                    *self.full.lock().unwrap_or_else(PoisonError::into_inner) =
                        Arc::downgrade(&image);
                }
                image
            }
        };
        Ok(Image::new(image, &self.path))
    }

    fn decode_raw(
        &self,
        ctx: &Context,
        decode: Decode,
        keep: bool,
        cancel: Cancel<'_>,
    ) -> Result<CameraImage> {
        let raw = rawmakase_decode::photo::open(&self.path, ctx.lenses()).map_err(Error::of)?;
        // The file read is still the one opened: one replaced before it was
        // read is neither decoded as this photo nor kept under its key.
        self.still_the_file_opened()?;
        let demosaic = Demosaic::default();
        let image = match (decode, ctx.decoded()) {
            (Decode::Half, _) => raw.develop(camera_data::Decode::Half, cancel),
            // Keyed by the file as opened, not as it is now.
            (Decode::Full, Some(cache)) => {
                FullSize::of_version(self.identity.as_ref(), demosaic, cache.clone())
                    .recovered(raw, keep, cancel)
            }
            (Decode::Full, None) => raw
                .develop(camera_data::Decode::Full(demosaic), cancel)
                .and_then(|image| quality::with_recovered(image, cancel)),
        }
        .map_err(Error::of)?;
        // A cache's copy is read without a check of its own.
        cancelled(cancel, "Decoding cancelled")?;
        Ok(image)
    }

    /// Refuses a file that is no longer the one opened: one replaced or
    /// changed since would be decoded in place of the photo shown.
    pub(crate) fn still_the_file_opened(&self) -> Result<()> {
        let Some(opened) = &self.identity else {
            return Ok(());
        };
        match Identity::read(&self.path) {
            // The same photo even when only its time changed (`touch`, a
            // cloud mount's rounding), as the catalog judges it.
            Ok(now) if now.same_photo(opened) => Ok(()),
            Ok(_) => Err(Error::of(
                rawmakase_model::error::Changed::new(format!(
                    "{} changed since it was opened",
                    self.path.display()
                ))
                .into(),
            )),
            Err(e) => Err(Error::of(e)),
        }
    }
}

impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Photo")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}
