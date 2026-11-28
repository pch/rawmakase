//! A decoded image: a photo's pixels, ready to render.
use crate::Recipe;
use rawmakase_model::camera_data::CameraImage;
use rayon::prelude::*;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// A photo's decoded pixels, made by [`Photo::decode`](crate::Photo::decode).
///
/// Its buffers are not an app's to reach into: a renderer and an export take the
/// handle, and [`Image::stats`] says what a caller may want to know of the
/// decode. Cheap to clone, the pixels are shared; `Send` and `Sync`.
#[derive(Clone)]
pub struct Image {
    image: Arc<CameraImage>,
    source: Arc<Path>,
    highest: Arc<OnceLock<f32>>,
}

/// What a decode measured, as plain numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeStats {
    /// The decoded image's width in pixels, before orientation and crop.
    pub width: u32,
    /// Its height in pixels, before orientation and crop.
    pub height: u32,
    /// Whether this is a reduced decode ([`Decode::Half`](crate::Decode::Half)
    /// of a mosaic), not every pixel of the photo.
    pub reduced: bool,
    /// What the decoder multiplied the samples by to bring them to scale. Always
    /// 1 with RAWmakase's own decoders; here because the command line prints it.
    pub scale: f32,
    /// How many of the sensor's samples were at their highest level: clipped.
    pub clipped_samples: u32,
    /// The highest value of any channel of the decoded image. The samples are
    /// white balanced, so a clipped channel reads above 1, the sensor's white
    /// level.
    pub highest_level: f32,
}

impl Image {
    pub(crate) fn new(image: Arc<CameraImage>, source: &Path) -> Self {
        Self {
            image,
            source: source.into(),
            highest: Arc::default(),
        }
    }

    pub(crate) fn camera(&self) -> &Arc<CameraImage> {
        &self.image
    }

    /// The file this image was decoded from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The decode's statistics. The highest level is measured the first time it
    /// is asked for (one pass over the pixels) and kept.
    pub fn stats(&self) -> DecodeStats {
        let image = &self.image;
        let highest = *self.highest.get_or_init(|| {
            let pixels = image.pixels.range(0..image.pixels.len());
            pixels
                .par_iter()
                .map(|p| p[0].max(p[1]).max(p[2]))
                .reduce(|| 0., f32::max)
        });
        DecodeStats {
            width: image.width,
            height: image.height,
            reduced: image.fast,
            scale: image.scale_factor,
            clipped_samples: image.scale_clipped,
            highest_level: highest,
        }
    }

    /// The size in pixels `recipe` renders this image at in full: oriented,
    /// cropped and transformed, as the renderer resolves it. A 100% region
    /// ([`View::Region`](crate::View::Region)) is given in these pixels.
    ///
    /// [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) for a recipe that
    /// does not validate.
    pub fn output_size(&self, recipe: &Recipe) -> crate::Result<(u32, u32)> {
        self.size_at(recipe, 0)
    }

    /// The size in pixels `recipe` renders this image at in `view`: a Fit's
    /// whole photo, its long edge at most the view's, or a region's own size.
    pub fn size_in(&self, recipe: &Recipe, view: crate::View) -> crate::Result<(u32, u32)> {
        match view {
            crate::View::Fit(edge) => self.size_at(recipe, edge),
            crate::View::Region([_, _, w, h]) => Ok((w, h)),
        }
    }

    pub(crate) fn size_at(&self, recipe: &Recipe, max_edge: u32) -> crate::Result<(u32, u32)> {
        rawmakase_engine::develop::quality::rendered_size(&self.image, &recipe.0, max_edge)
            .map_err(|e| crate::Error::of(rawmakase_model::error::Invalid::from_error(e).into()))
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("source", &self.source)
            .field("width", &self.image.width)
            .field("height", &self.image.height)
            .finish_non_exhaustive()
    }
}
