//! Small renders of one photo under many settings, such as the Presets panel's
//! tiles: each preset applied to the open photo.
//!
//! [`ThumbnailRenderer`] renders from a reduced copy of the photo
//! ([`SOURCE_EDGE`] pixels long), made once per decode, never from the photo
//! itself: a tile then costs what a small render costs, whatever the photo's size.
//! The copy is made as Auto tone makes its own: the photo's highlights recovered,
//! then reduced, with its `recovered` slot pointing at itself so they are not
//! recovered twice. It is marked `fast`, so sharpening, grain, noise reduction and
//! Texture are sized in the photo's pixels, as for a reduced decode.
//!
//! The copy shares the photo's metadata, and with it the photo's Auto CA
//! measurement ([`crate::lens::auto_ca::prime`]). That is always measured on the
//! photo, before a tile that needs it renders, never on the copy.
//!
//! The renderer keeps its own [`PreviewRenderer`] on the CPU, so tiles never take
//! the main view's cached stages or its GPU. AI Denoise is not applied (a reduced
//! image never fits its source). An HDR edit's tile is its SDR rendition.
use super::{PreviewRenderer, pipeline::preview, quality::recovered};
use crate::camera_data::CameraImage;
use crate::model::recipe::Recipe;
use crate::rendered::Rendered;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::sync::{Arc, Weak};

/// The long edge of the reduced copy tiles render from. Never below
/// [`super::local_tone::MAP_EDGE`], so the copy measured for Shadows, Highlights
/// and Clarity keeps the detail the photo's own measurement sees.
pub const SOURCE_EDGE: u32 = 1024;
const _: () = assert!(SOURCE_EDGE >= super::local_tone::MAP_EDGE);

/// Renders small views of whatever photo it is handed, from a reduced copy kept
/// for the latest photo. One per worker: it is not shared between threads.
#[derive(Default)]
pub struct ThumbnailRenderer {
    source: Option<Source>,
    renderer: PreviewRenderer,
}

/// The reduced copy and the photo it was made from, by its allocation, held
/// weakly so the copy never keeps the photo.
struct Source {
    photo: Weak<CameraImage>,
    copy: Arc<CameraImage>,
}

impl ThumbnailRenderer {
    /// Measures `photo`'s Auto CA when `recipe` asks for it, so a render of the
    /// reduced copy uses the photo's measurement. Idempotent and quick once
    /// measured; [`Self::render`] does it too. A caller that renders on a slow
    /// pool calls this first on its own thread, so the measurement, which the main
    /// view shares, is never made at background priority.
    pub fn prime(photo: &CameraImage, recipe: &Recipe) {
        if recipe.as_rendered().resolved(&photo.metadata).lens_ca {
            crate::lens::auto_ca::prime(photo);
        }
    }

    /// `recipe` rendered from `photo`'s reduced copy, its whole crop scaled so its
    /// short edge is `cover` pixels (for a square tile cut from its middle), within
    /// the copy's size. Stops with an error once `cancel` is set.
    pub fn render(
        &mut self,
        photo: &Arc<CameraImage>,
        recipe: &Recipe,
        cover: u32,
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        cancel.check("Cancelled")?;
        Self::prime(photo, recipe);
        let copy = self.copy_of(photo, cancel)?;
        let shown = recipe.as_rendered();
        let geometry = super::Geometry::new(&copy, &shown, 0);
        let (long, short) = (
            geometry.width.max(geometry.height).max(1),
            geometry.width.min(geometry.height).max(1),
        );
        let edge = (u64::from(cover.max(1)) * u64::from(long)).div_ceil(u64::from(short));
        let edge = (edge.min(u64::from(long))) as u32;
        self.renderer.render(&copy, recipe, edge, None, cancel)
    }

    /// The reduced copy of `photo`, made when the one kept is of another photo.
    fn copy_of(
        &mut self,
        photo: &Arc<CameraImage>,
        cancel: Cancel<'_>,
    ) -> Result<Arc<CameraImage>> {
        let kept = self
            .source
            .as_ref()
            .is_some_and(|s| std::ptr::eq(s.photo.as_ptr(), Arc::as_ptr(photo)));
        if !kept {
            // Let go of the last photo's copy and caches before making the next.
            self.source = None;
            self.renderer = PreviewRenderer::default();
            self.source = Some(Source {
                photo: Arc::downgrade(photo),
                copy: Arc::new(reduced(photo, cancel)?),
            });
        }
        Ok(self.source.as_ref().expect("kept or made").copy.clone())
    }
}

/// `photo` with its highlights recovered, reduced to [`SOURCE_EDGE`] and marked
/// `fast`; its own recovered image is itself.
fn reduced(photo: &CameraImage, cancel: Cancel<'_>) -> Result<CameraImage> {
    let mut small = preview(&*recovered(photo, cancel)?, SOURCE_EDGE);
    small.fast = true;
    // One buffer for the copy and its recovered image.
    small.pixels.share();
    let _ = small.recovered.set(Arc::new(small.clone()));
    Ok(small)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Metadata;
    use std::sync::atomic::AtomicBool;

    /// A `w` × `h` photo with fine detail, a gradient and some clipped highlights.
    fn photo(w: u32, h: u32) -> CameraImage {
        CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    let v = 0.25
                        + 0.15 * (x * 0.013).sin() * (y * 0.011).cos()
                        + 0.04 * (x * 0.4).sin() * (y * 0.37).sin();
                    let bright = if (x - w as f32 * 0.7).hypot(y - h as f32 * 0.3) < 40. {
                        1.2
                    } else {
                        0.
                    };
                    [v * 1.1 + bright, v + bright, v * 0.7 + x / w as f32 * 0.2]
                })
                .collect(),
            metadata: Metadata {
                width: w,
                height: h,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        }
    }

    /// The copy is reduced to the source edge, marked `fast`, keeps the photo's
    /// metadata and is its own recovered image, so highlights are not recovered
    /// a second time from the recovered pixels.
    #[test]
    fn the_copy_is_reduced_fast_and_already_recovered() {
        let im = Arc::new(photo(2400, 1600));
        let mut t = ThumbnailRenderer::default();
        t.render(&im, &Recipe::default(), 64, Cancel::NEVER)
            .unwrap();
        let copy = &t.source.as_ref().unwrap().copy;
        assert_eq!((copy.width, copy.height), (SOURCE_EDGE, 683));
        assert!(copy.fast);
        assert_eq!(copy.metadata.width, 2400);
        assert!(copy.photo_scale() < 0.5);
        let own = copy.recovered.get().unwrap();
        assert!(own.pixels.shares_buffer(&copy.pixels));
        // Reduced from the recovered photo, not the photo as decoded.
        let from_recovered = preview(&recovered(&im, Cancel::NEVER).unwrap(), SOURCE_EDGE);
        assert_eq!(copy.pixels, from_recovered.pixels);
        assert_ne!(copy.pixels, preview(&im, SOURCE_EDGE).pixels);
        // A photo smaller than the source edge is copied at its own size.
        let small = Arc::new(photo(300, 200));
        t.render(&small, &Recipe::default(), 64, Cancel::NEVER)
            .unwrap();
        let copy = &t.source.as_ref().unwrap().copy;
        assert_eq!((copy.width, copy.height), (300, 200));
    }

    /// The copy is kept for the same photo's allocation and made again for
    /// another, even one with the same pixels; the old copy goes with it.
    #[test]
    fn another_photo_allocation_makes_a_new_copy() {
        let a = Arc::new(photo(1600, 1200));
        let mut t = ThumbnailRenderer::default();
        let first = t.render(&a, &Recipe::default(), 96, Cancel::NEVER).unwrap();
        let copy = t.source.as_ref().unwrap().copy.clone();
        // Another render of the same photo reuses the copy: its buffer is the same.
        let exposed = Recipe {
            exposure: 1.,
            ..Default::default()
        };
        t.render(&a, &exposed, 96, Cancel::NEVER).unwrap();
        assert!(Arc::ptr_eq(&t.source.as_ref().unwrap().copy, &copy));
        // The same pixels in another allocation: a new decode, a new copy.
        let b = Arc::new(photo(1600, 1200));
        let again = t.render(&b, &Recipe::default(), 96, Cancel::NEVER).unwrap();
        assert!(!Arc::ptr_eq(&t.source.as_ref().unwrap().copy, &copy));
        assert!(std::ptr::eq(
            t.source.as_ref().unwrap().photo.as_ptr(),
            Arc::as_ptr(&b)
        ));
        assert_eq!(first.pixels, again.pixels);
    }

    /// A tile with Remove Chromatic Aberration rendered first measures it on the
    /// photo, as rendering the photo would, never on the reduced copy, whose
    /// metadata shares the measurement with the photo and so with the main view.
    #[test]
    fn a_lens_ca_tile_primes_auto_ca_from_the_photo() {
        // Soft dots whose red is magnified about the centre by 0.08% and blue
        // shrunk by 0.05%, as `auto_ca`'s own tests measure.
        let mut im = photo(1500, 1000);
        let (cx, cy) = (750., 500.);
        let value = |x: f32, y: f32| {
            let (gx, gy) = ((x / 37.).fract() - 0.5, (y / 37.).fract() - 0.5);
            let d = (gx * gx + gy * gy).sqrt() * 37.;
            0.1 + 0.6 * (1. / (1. + ((d - 11.) / 1.2).exp()))
        };
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % 1500) as f32, (i as u32 / 1500) as f32);
            let (dx, dy) = (x + 0.5 - cx, y + 0.5 - cy);
            let at = |k: f32| value(cx + dx / k + 1000., cy + dy / k + 1000.);
            *p = [at(1.0008), at(1.), at(0.9995)];
        }
        let im = Arc::new(im);
        let expected = {
            let fresh = CameraImage {
                metadata: Metadata {
                    lateral_ca: Default::default(),
                    ..im.metadata.clone()
                },
                ..(*im).clone()
            };
            crate::lens::auto_ca::prime(&fresh);
            fresh.metadata.lateral_ca.get().cloned().unwrap()
        };
        assert!(expected.is_some(), "the photo shows a measurable CA");
        assert!(im.metadata.lateral_ca.get().is_none());
        let r = Recipe {
            lens_ca: true,
            ..Default::default()
        };
        assert!(r.as_rendered().resolved(&im.metadata).lens_ca);
        ThumbnailRenderer::default()
            .render(&im, &r, 64, Cancel::NEVER)
            .unwrap();
        let measured = im.metadata.lateral_ca.get().cloned().unwrap();
        assert_eq!(
            format!("{measured:?}"),
            format!("{expected:?}"),
            "measured on the photo"
        );
        // The copy's own estimate would differ: it is what priming avoids.
        let copy = preview(&im, SOURCE_EDGE);
        let from_copy = crate::lens::auto_ca::estimate(
            &copy.pixels.range(0..copy.pixels.len()),
            copy.width,
            copy.height,
            crate::lens::auto_ca::centre(&copy.metadata),
        );
        assert_ne!(format!("{from_copy:?}"), format!("{expected:?}"));
        // Without the setting nothing is measured.
        let plain = Arc::new(photo(400, 300));
        ThumbnailRenderer::default()
            .render(&plain, &Recipe::default(), 64, Cancel::NEVER)
            .unwrap();
        assert!(plain.metadata.lateral_ca.get().is_none());
    }

    /// A tile is close to Develop's Fit render of the whole photo at the same
    /// size, with detail, local and spatial effects, a crop and straightening.
    #[test]
    fn a_tile_approximates_the_fit_render() {
        let im = Arc::new(photo(2400, 1600));
        let mut r = Recipe {
            sharpening: 0.8,
            exposure: 0.3,
            shadows: 0.4,
            highlights: -0.5,
            straighten: 2.,
            crop: [0.05, 0.1, 0.95, 0.9],
            ..Default::default()
        };
        r.effects.clarity = 0.4;
        r.effects.texture = 0.3;
        r.effects.vignette = -0.4;
        let mut t = ThumbnailRenderer::default();
        for cover in [96, 160, 290] {
            let tile = t.render(&im, &r, cover, Cancel::NEVER).unwrap();
            assert!(tile.width.min(tile.height) >= cover, "{cover}");
            let fit = PreviewRenderer::default()
                .render(&im, &r, tile.width.max(tile.height), None, Cancel::NEVER)
                .unwrap();
            assert_eq!((fit.width, fit.height), (tile.width, tile.height));
            let error = tile
                .pixels
                .iter()
                .flatten()
                .zip(fit.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / (tile.pixels.len() * 3) as f32;
            // 0.0018, 0.0022 and 0.0034 measured.
            assert!(error < 0.006, "cover {cover}: mean error {error}");
        }
    }

    /// A cancelled render stops with an error, before and while it renders.
    #[test]
    fn a_cancelled_tile_stops() {
        let im = Arc::new(photo(800, 600));
        let cancel = AtomicBool::new(true);
        let mut t = ThumbnailRenderer::default();
        assert!(
            t.render(&im, &Recipe::default(), 64, Cancel::new(&cancel))
                .is_err()
        );
        // Cancelled after the copy is made: the render itself stops.
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        let copy = t.copy_of(&im, Cancel::NEVER).unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            t.renderer
                .render(&copy, &Recipe::default(), 64, None, Cancel::new(&cancel))
                .is_err()
        );
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(
            t.render(&im, &Recipe::default(), 64, Cancel::new(&cancel))
                .is_ok()
        );
    }
}
