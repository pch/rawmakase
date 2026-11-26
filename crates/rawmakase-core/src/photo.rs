//! A photo: a file opened for developing, what the library knows of it
//! before its pixels are decoded, and its decode.
use crate::{Context, Error, Image, Result};
use rawmakase_cancel::Cancel;
use rawmakase_decode::decode::{DecodePolicy, FullSize};
use rawmakase_decode::photo::Opened;
use rawmakase_engine::develop::quality;
use rawmakase_interop::edits;
use rawmakase_model::camera_data::{self, CameraImage, Demosaic, Metadata};
use rawmakase_model::camera_profiles::CameraProfile;
use rawmakase_model::model::recipe::Recipe;
use rawmakase_model::storage::Identity;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
/// It keeps no RAW file's bytes: each [`Photo::decode`] reads the file again,
/// and refuses it as [`ErrorKind::Changed`](crate::ErrorKind::Changed) when it
/// is no longer the file that was opened.
pub struct Photo {
    ctx: Context,
    path: PathBuf,
    metadata: Metadata,
    profiles: Vec<Arc<CameraProfile>>,
    profile_errors: Vec<String>,
    /// The file as it was opened; `None` when its identity could not be read,
    /// and then every decode is of whatever is there.
    identity: Option<Identity>,
    /// A rendered file (JPEG, PNG, TIFF…), which opening decodes.
    rendered: Option<Arc<CameraImage>>,
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
        Ok(Photo {
            ctx: self.clone(),
            path: path.to_path_buf(),
            metadata,
            profiles,
            profile_errors: listed.errors,
            identity,
            rendered: match opened {
                // Dropped: a decode reads the file again.
                Opened::Raw(_) => None,
                Opened::Rendered(image) => Some(Arc::new(image)),
            },
        })
    }
}

impl Photo {
    /// The file this photo was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file says of itself: camera, lens, exposure, size, white
    /// balance.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// The camera profiles that fit this photo, sorted by name.
    pub fn profiles(&self) -> &[Arc<CameraProfile>] {
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
    /// from there next time. Each decode reads the file again.
    ///
    /// Errors: [`ErrorKind::Changed`](crate::ErrorKind::Changed) when the file
    /// is no longer the one opened, [`ErrorKind::Missing`](crate::ErrorKind::Missing)
    /// when it is gone, and [`ErrorKind::Cancelled`](crate::ErrorKind::Cancelled).
    pub fn decode(&self, decode: Decode, cancel: Cancel<'_>) -> Result<Image> {
        self.still_the_file_opened()?;
        let image = match &self.rendered {
            Some(image) => image.clone(),
            None => Arc::new(self.decode_raw(decode, cancel).map_err(Error::of)?),
        };
        Ok(Image::new(image, &self.path))
    }

    fn decode_raw(&self, decode: Decode, cancel: Cancel<'_>) -> anyhow::Result<CameraImage> {
        let lenses = self.ctx.lenses();
        let open = || rawmakase_decode::photo::open(&self.path, lenses);
        let demosaic = Demosaic::default();
        match (decode, self.ctx.decoded()) {
            (Decode::Half, _) => open()?.develop(camera_data::Decode::Half, cancel),
            (Decode::Full, Some(cache)) => {
                FullSize::with_cache(&self.path, demosaic, cache.clone())
                    .get(open, DecodePolicy::Show, cancel)?
                    .ok_or_else(|| anyhow::anyhow!("The decode cache gave no image"))
            }
            (Decode::Full, None) => quality::with_recovered(
                open()?.develop(camera_data::Decode::Full(demosaic), cancel)?,
                cancel,
            ),
        }
    }

    /// Refuses a file that is no longer the one opened: one replaced or
    /// changed since would be decoded in place of the photo shown.
    pub(crate) fn still_the_file_opened(&self) -> Result<()> {
        let Some(opened) = &self.identity else {
            return Ok(());
        };
        match Identity::read(&self.path) {
            Ok(now) if now == *opened => Ok(()),
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
