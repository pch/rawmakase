//! Rendering an image with a recipe to pixels, on the CPU, as a view of an
//! editor shows it: the whole photo at a size, or a region at 100%, with a
//! reduced draft first when the full frame will be slow.
use crate::{Context, Error, Image, Result};
use rawmakase_cancel::Cancel;
use rawmakase_engine::develop::PreviewRenderer;
use rawmakase_engine::develop::draft::{Drafts, draft_edge};
use rawmakase_model::camera_data::CameraImage;
use rawmakase_model::model::recipe::Recipe;
use rawmakase_model::rendered::{Rendered, unit_to_u8};
use rayon::prelude::*;
use std::sync::{Arc, Weak};
use std::time::Instant;

/// What part of a photo a render shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The whole photo, its long edge at most this many pixels; 0 for full
    /// size.
    Fit(u32),
    /// A region at 100%: `[x, y, width, height]` in the pixels of the photo
    /// rendered in full ([`Image::output_size`]).
    Region([u32; 4]),
}

/// Rendered pixels: encoded sRGB, rows from the top.
pub struct Frame {
    rendered: Rendered,
    view: View,
    draft: bool,
}

impl Frame {
    /// The frame's width in pixels.
    pub fn width(&self) -> u32 {
        self.rendered.width
    }

    /// The frame's height in pixels.
    pub fn height(&self) -> u32 {
        self.rendered.height
    }

    /// The view it shows. A region's draft is smaller than the region, and is
    /// stretched over it.
    pub fn view(&self) -> View {
        self.view
    }

    /// Whether this is a reduced draft, which the view's full frame follows.
    pub fn is_draft(&self) -> bool {
        self.draft
    }

    /// The pixels as 8-bit RGBA, opaque: what a canvas or a texture takes.
    pub fn rgba8(&self) -> Vec<u8> {
        let mut out = vec![255; self.rendered.pixels.len() * 4];
        // Rows of a megapixel frame convert in a millisecond or two across
        // cores, and in several on one.
        out.par_chunks_mut(4 * 4096)
            .zip(self.rendered.pixels.par_chunks(4096))
            .for_each(|(out, pixels)| {
                for (out, pixel) in out.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
                    out[0] = unit_to_u8(pixel[0]);
                    out[1] = unit_to_u8(pixel[1]);
                    out[2] = unit_to_u8(pixel[2]);
                }
            });
        out
    }

    /// The pixels as 8-bit RGB.
    pub fn rgb8(&self) -> Vec<u8> {
        self.rendered.rgb8()
    }

    /// The pixels as 16-bit RGB.
    pub fn rgb16(&self) -> Vec<u16> {
        self.rendered.rgb16()
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.rendered.width)
            .field("height", &self.rendered.height)
            .field("view", &self.view)
            .field("draft", &self.draft)
            .finish_non_exhaustive()
    }
}

/// A renderer for one view of a photo being edited, on the CPU.
///
/// It keeps what makes the next render of the same photo fast: the photo
/// reduced for Fit sizes, the stages an edit did not change, and how long its
/// last full frames took, which says whether a draft is due
/// ([`Renderer::draft_due`]): the desktop's rule. It belongs to the thread
/// that renders, and one photo at a time. Made by [`Context::renderer`].
///
/// It doesn't run AI Denoise yet: a recipe with it on renders as if it were
/// off, where [`Context::export`](crate::Context) refuses it.
pub struct Renderer {
    preview: PreviewRenderer,
    fit: Drafts,
    region: Drafts,
    /// The image and view of the last full frame, without keeping the image.
    last: Option<(Weak<CameraImage>, View)>,
}

impl Context {
    /// A renderer, to be kept by the thread that renders and used for every
    /// frame of a view.
    pub fn renderer(&self) -> Renderer {
        Renderer {
            preview: PreviewRenderer::default(),
            fit: Drafts::default(),
            region: Drafts::default(),
            last: None,
        }
    }

    /// Renders `image` with `recipe` once, keeping nothing: a thumbnail, a
    /// preview of a photo not being edited. An editor keeps a [`Renderer`].
    pub fn render(
        &self,
        image: &Image,
        recipe: &Recipe,
        view: View,
        cancel: Cancel<'_>,
    ) -> Result<Frame> {
        self.renderer().render(image, recipe, view, cancel)
    }
}

impl Renderer {
    /// Whether the next frame of `view` should be preceded by a reduced draft
    /// ([`Renderer::render_draft`]), and its size: while the view already
    /// shown is edited, unless its last full frame was quick, and for the
    /// whole photo only when it is large. A new photo or view goes straight to
    /// its full frame. A whole photo's draft is given as its own size; a
    /// region's as the region's, which it covers at half resolution or less.
    pub fn draft_due(&self, image: &Image, recipe: &Recipe, view: View) -> Option<(u32, u32)> {
        let editing = self.last.as_ref().is_some_and(|(shown, at)| {
            std::ptr::eq(shown.as_ptr(), Arc::as_ptr(image.camera()))
                && match (at, view) {
                    (View::Fit(a), View::Fit(b)) => *a == b,
                    (View::Region(_), View::Region(_)) => true,
                    _ => false,
                }
        });
        match view {
            View::Fit(edge) => {
                if !self.fit.wanted(editing) {
                    return None;
                }
                // An invalid recipe has no draft: its render says why.
                let (w, h) = image.size_at(recipe, edge).ok()?;
                let reduced = draft_edge(w, h)?;
                image.size_at(recipe, reduced).ok()
            }
            View::Region([_, _, w, h]) => self.region.wanted(editing).then_some((w, h)),
        }
    }

    /// Renders the reduced draft of `view`: the whole photo at the draft's
    /// size, or the region at half resolution or less, which the caller
    /// stretches over it. Shown until the full frame comes. A whole photo
    /// small enough to need no draft is rendered at its size, and is no
    /// draft.
    pub fn render_draft(
        &mut self,
        image: &Image,
        recipe: &Recipe,
        view: View,
        cancel: Cancel<'_>,
    ) -> Result<Frame> {
        recipe.validate().map_err(invalid)?;
        let (rendered, draft) = match view {
            View::Fit(edge) => {
                let (w, h) = image.size_at(recipe, edge)?;
                let reduced = draft_edge(w, h);
                let rendered = self.preview.render(
                    image.camera(),
                    recipe,
                    reduced.unwrap_or(edge),
                    None,
                    cancel,
                );
                (rendered, reduced.is_some())
            }
            View::Region(region) => {
                let rendered =
                    self.preview
                        .render_region_preview(image.camera(), recipe, region, cancel);
                (rendered, true)
            }
        };
        Ok(Frame {
            rendered: rendered.map_err(Error::of)?,
            view,
            draft,
        })
    }

    /// Renders `view` of `image` with `recipe`. A whole photo takes from a few
    /// milliseconds to a few tenths of a second with its size; the first render
    /// of a photo also reduces it.
    ///
    /// Errors: [`ErrorKind::Invalid`](crate::ErrorKind::Invalid) for a recipe
    /// that does not validate, and
    /// [`ErrorKind::Cancelled`](crate::ErrorKind::Cancelled) once `cancel` is
    /// set, which a caller drops without showing.
    pub fn render(
        &mut self,
        image: &Image,
        recipe: &Recipe,
        view: View,
        cancel: Cancel<'_>,
    ) -> Result<Frame> {
        recipe.validate().map_err(invalid)?;
        let (max_edge, region) = match view {
            View::Fit(max_edge) => (max_edge, None),
            View::Region(region) => (0, Some(region)),
        };
        let started = Instant::now();
        let rendered = self
            .preview
            .render(image.camera(), recipe, max_edge, region, cancel)
            .map_err(Error::of)?;
        // A frame on the CPU is finished when it returns; turning it into
        // RGBA is the caller's, and quick.
        let drafts = match view {
            View::Fit(_) => &mut self.fit,
            View::Region(_) => &mut self.region,
        };
        drafts.rendered(started.elapsed(), true);
        self.last = Some((Arc::downgrade(image.camera()), view));
        Ok(Frame {
            rendered,
            view,
            draft: false,
        })
    }
}

fn invalid(e: anyhow::Error) -> Error {
    Error::of(rawmakase_model::error::Invalid::from_error(e).into())
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer").finish_non_exhaustive()
    }
}
