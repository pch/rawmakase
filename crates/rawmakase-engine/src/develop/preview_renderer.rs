//! Stateful desktop preview backend. Export remains on the reference CPU path.
use super::{
    gpu,
    pyramid::Pyramid,
    quality::{self, Output},
    stage_cache::StageCache,
};
use crate::rendered::Rendered;
use crate::{camera_data::CameraImage, model::recipe::Recipe};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::sync::Arc;

#[derive(Default)]
pub struct PreviewRenderer {
    backend: Backend,
    /// Resolution pyramid of the current photo's recovered image, for Fit and
    /// zoomed-out renders. Holding the recovered image keeps its identity unique.
    pyramid: Option<Pyramid>,
    /// Stage results reused while only color and tone change.
    cache: StageCache,
    /// The recovered image with spot removal applied, updated tile by tile.
    retouch: super::retouch::RetouchCache,
    /// The AI Denoise source the next renders use ([`Self::set_ai`]).
    ai: Option<super::AiSource>,
}
/// The optional GPU and why it is not used.
#[derive(Default)]
pub(crate) struct Backend {
    pub(crate) gpu: Option<gpu::Processor>,
    fallback: Option<String>,
    used_gpu: bool,
    /// Why the photo is not kept on the device, after that path failed (for example
    /// for lack of memory); the rest of the GPU path keeps working.
    pub(crate) resident_fallback: Option<String>,
}
/// What preview stages keep between renders: the stage cache and the GPU backend, and
/// the display a render may be presented to.
pub(crate) struct Stages<'a> {
    pub(crate) cache: &'a mut StageCache,
    pub(crate) retouch: &'a mut super::retouch::RetouchCache,
    pub(crate) backend: &'a mut Backend,
    pub(crate) display: Option<&'a gpu::Display>,
}
/// Pixel budget of a reduced 100% drag preview.
const PREVIEW_PIXELS: u64 = 600_000;
/// The whole photo within `max_edge`, as Develop's Fit draws it (from the smallest
/// pyramid level with a pixel per output pixel), on the CPU and keeping nothing
/// between calls: previews of many photos, such as the Library's edited ones. Stops
/// once `cancel` is set. Exports render at full resolution with [`super::render`].
pub fn render_fitted(
    image: &CameraImage,
    recipe: &Recipe,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<Rendered> {
    PreviewRenderer::default().render(image, recipe, max_edge, None, cancel)
}
impl PreviewRenderer {
    /// A hardware device is optional; failure leaves a fully working CPU renderer.
    pub fn with_gpu() -> Self {
        Self::with_processor(gpu::Processor::new())
    }
    /// With `gpu`, or on the CPU alone when it failed.
    pub fn with_processor(gpu: Result<gpu::Processor>) -> Self {
        let backend = match gpu {
            Ok(gpu) => Backend {
                gpu: Some(gpu),
                ..Backend::default()
            },
            Err(error) => Backend {
                fallback: Some(error.to_string()),
                ..Backend::default()
            },
        };
        Self {
            backend,
            ..Self::default()
        }
    }
    /// Lets go of the noise-reduced image the next render replaces (the pyramid built on
    /// it, its upload) before the next is made, so two full-size images of the photo
    /// are not alive at once while AI Denoise's Amount or noise reduction moves.
    fn release_replaced(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        ai: Option<&super::AiSource>,
        cancel: Cancel<'_>,
    ) -> Result<()> {
        use super::recipe::RenderedRecipe;
        let recovered = quality::recovered(image, cancel)?;
        let d = recipe.denoise_with(ai.filter(|a| a.fits(image)));
        let Some(old) = self.retouch.replaced_by(&recovered, d.as_ref()) else {
            return Ok(());
        };
        if self
            .pyramid
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p.source(), &old) || self.retouch.holds(p.source()))
        {
            self.pyramid = None;
        }
        self.retouch.forget_denoised();
        if let Some(gpu) = &mut self.backend.gpu {
            gpu.forget_resident(&old);
        }
        Ok(())
    }
    /// The AI Denoise source of the photo the next renders show, as the render job
    /// captured it; `None` renders without AI Denoise (no result yet). A source of
    /// another size than the photo is ignored.
    pub fn set_ai(&mut self, ai: Option<super::AiSource>) {
        self.ai = ai;
    }
    pub fn adapter_name(&self) -> Option<&str> {
        self.backend.gpu.as_ref().map(gpu::Processor::name)
    }
    /// The GPU, while it works.
    pub fn gpu(&self) -> Option<&gpu::Processor> {
        self.backend.gpu.as_ref()
    }
    pub fn fallback_reason(&self) -> Option<&str> {
        self.backend.fallback.as_deref()
    }
    /// Whether the last render used the GPU for any stage.
    pub fn used_gpu(&self) -> bool {
        self.backend.used_gpu
    }
    pub fn render(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        max_edge: u32,
        region: Option<[u32; 4]>,
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        self.render_to(image, recipe, max_edge, region, cancel, None)
            .map(Output::pixels)
    }
    /// As [`Self::render`], presented into a texture for `display` when the GPU renders
    /// the recipe, and otherwise as pixels.
    pub fn render_to(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        max_edge: u32,
        region: Option<[u32; 4]>,
        cancel: Cancel<'_>,
        display: Option<&gpu::Display>,
    ) -> Result<Output> {
        let shown = recipe.as_rendered();
        let recipe = shown.as_ref();
        self.backend.used_gpu = false;
        anyhow::ensure!(!cancel.is_cancelled(), "Render superseded");
        let recipe = &recipe.checked()?;
        let resolved = quality::resolve(image, recipe);
        if region.is_none()
            && max_edge > 0
            && let Some(out) = self.render_fit(image, &resolved, max_edge, cancel, display)?
        {
            return Ok(out);
        }
        let ai = self.ai.clone();
        self.release_replaced(image, &resolved.recipe, ai.as_ref(), cancel)?;
        quality::render_preview(
            image,
            &resolved,
            ai.as_ref(),
            max_edge,
            region,
            crate::color::OutputSpace::Srgb,
            cancel,
            Some(&mut self.stages(display)),
        )
    }
    fn stages<'a>(&'a mut self, display: Option<&'a gpu::Display>) -> Stages<'a> {
        Stages {
            cache: &mut self.cache,
            retouch: &mut self.retouch,
            backend: &mut self.backend,
            display,
        }
    }
    /// Fit and zoomed-out views from the smallest pyramid level with at least one
    /// pixel per output pixel. `None` when the output is the full resolution.
    fn render_fit(
        &mut self,
        image: &CameraImage,
        resolved: &quality::Resolved,
        max_edge: u32,
        cancel: Cancel<'_>,
        display: Option<&gpu::Display>,
    ) -> Result<Option<Output>> {
        let full = resolved.geometry;
        let long = full.width.max(full.height);
        if long <= max_edge {
            return Ok(None);
        }
        let size = quality::output_size(full.width, full.height, max_edge);
        let needed = size.0.max(size.1) as f32 * image.width.max(image.height) as f32 / long as f32;
        let (level, source) = self.level(image, resolved, needed, cancel)?;
        let region = [0, 0, size.0, size.1];
        let mut stages = self.stages(display);
        let recipe = &resolved.recipe;
        quality::render_level(&level, &source, recipe, size, region, cancel, &mut stages).map(Some)
    }
    /// A 100% `region` at half resolution or less, from the pyramid: immediate
    /// feedback while dragging, before the full-resolution region. The viewport
    /// stretches it over the region.
    pub fn render_region_preview(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        region: [u32; 4],
        cancel: Cancel<'_>,
    ) -> Result<Rendered> {
        self.render_region_preview_to(image, recipe, region, cancel, None)
            .map(Output::pixels)
    }
    /// As [`Self::render_region_preview`], presented for `display` when possible.
    pub fn render_region_preview_to(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        region: [u32; 4],
        cancel: Cancel<'_>,
        display: Option<&gpu::Display>,
    ) -> Result<Output> {
        let shown = recipe.as_rendered();
        let recipe = shown.as_ref();
        self.backend.used_gpu = false;
        let recipe = &recipe.checked()?;
        let resolved = quality::resolve(image, recipe);
        let full = resolved.geometry;
        let [x, y, w, h] = region;
        anyhow::ensure!(
            w > 0
                && h > 0
                && x.checked_add(w).is_some_and(|r| r <= full.width)
                && y.checked_add(h).is_some_and(|b| b <= full.height),
            "Invalid viewport region"
        );
        // Halve until the preview has at most PREVIEW_PIXELS.
        let mut k = 1;
        while (w as u64 * h as u64) >> (2 * k) > PREVIEW_PIXELS && k < 4 {
            k += 1;
        }
        let div = |v: u32| v.div_ceil(1 << k).max(1);
        let size = (div(full.width), div(full.height));
        let (px, py) = (x >> k, y >> k);
        let (pw, ph) = (div(w).min(size.0 - px), div(h).min(size.1 - py));
        let needed = image.width.max(image.height).div_ceil(1 << k) as f32;
        let (level, source) = self.level(image, &resolved, needed, cancel)?;
        let region = [px, py, pw, ph];
        let mut stages = self.stages(display);
        let recipe = &resolved.recipe;
        quality::render_level(&level, &source, recipe, size, region, cancel, &mut stages)
    }
    /// The pyramid level for `needed` source pixels on the long edge, and the level-0
    /// image (recovered and retouched), building the pyramid when the photo changed and
    /// patching it where spot removal changed.
    fn level(
        &mut self,
        image: &CameraImage,
        resolved: &quality::Resolved,
        needed: f32,
        cancel: Cancel<'_>,
    ) -> Result<(Arc<CameraImage>, quality::Retouched)> {
        let ai = self.ai.clone();
        self.release_replaced(image, &resolved.recipe, ai.as_ref(), cancel)?;
        let source = resolved.source(image, ai.as_ref(), cancel, Some(&mut self.retouch))?;
        match &mut self.pyramid {
            Some(p) if Arc::ptr_eq(p.source(), &source.image) => {}
            Some(p) if self.retouch.changed_from(p.source()).is_some() => {
                let rects = self.retouch.changed_from(p.source()).unwrap().to_vec();
                p.update(source.image.clone(), &rects);
            }
            _ => self.pyramid = Some(Pyramid::new(source.image.clone())),
        }
        let pyramid = self.pyramid.as_mut().unwrap();
        let full = quality::Retouched {
            image: pyramid.source().clone(),
            unretouched: source.unretouched,
        };
        Ok((pyramid.level_for(needed), full))
    }
    #[cfg(test)]
    pub(crate) fn finish(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        sigma: f32,
        max_edge: u32,
        cancel: Cancel<'_>,
    ) -> Option<Rendered> {
        self.backend.finish(image, recipe, sigma, max_edge, cancel)
    }
}
impl Backend {
    pub(crate) fn has_gpu(&self) -> bool {
        self.gpu.is_some()
    }
    /// Runs `stage` on the GPU; a failure other than cancellation disables the GPU,
    /// so a failing device is not retried on every slider movement.
    /// Runs a stage of the resident path (see `quality::render_resident`); a failure
    /// other than cancellation turns that path off, not the GPU.
    pub(crate) fn run_resident<T>(
        &mut self,
        cancel: Cancel<'_>,
        stage: impl FnOnce(&mut gpu::Processor) -> Result<T>,
    ) -> Option<T> {
        if self.resident_fallback.is_some() {
            return None;
        }
        let gpu = self.gpu.as_mut()?;
        match stage(gpu) {
            Ok(out) => Some(out),
            Err(error) => {
                if !cancel.is_cancelled() {
                    self.resident_fallback = Some(error.to_string());
                }
                None
            }
        }
    }
    pub(crate) fn run<T>(
        &mut self,
        cancel: Cancel<'_>,
        stage: impl FnOnce(&mut gpu::Processor) -> Result<T>,
    ) -> Option<T> {
        let gpu = self.gpu.as_mut()?;
        match stage(gpu) {
            Ok(out) => {
                self.used_gpu = true;
                Some(out)
            }
            Err(error) => {
                if !cancel.is_cancelled() {
                    self.fallback = Some(error.to_string());
                    self.gpu = None;
                }
                None
            }
        }
    }
    pub(crate) fn finish(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        sigma: f32,
        max_edge: u32,
        cancel: Cancel<'_>,
    ) -> Option<Rendered> {
        self.run(cancel, |gpu| {
            gpu.finish(image, recipe, sigma, max_edge, cancel)
        })
    }
    /// Whether the GPU can develop `samples` with `params`; checked before [`Self::run`],
    /// so a region too large for the device renders on the CPU without turning the GPU
    /// off for the next one.
    fn fits(
        &self,
        samples: &super::pipeline::Samples,
        params: &super::pipeline::pixel_params::PixelParams,
    ) -> bool {
        self.gpu
            .as_ref()
            .is_some_and(|gpu| gpu.fits_develop(samples.width, samples.height, params))
    }
    pub(crate) fn develop(
        &mut self,
        samples: &Arc<super::pipeline::Samples>,
        params: &super::pipeline::pixel_params::PixelParams,
        cancel: Cancel<'_>,
    ) -> Option<Rendered> {
        if !self.fits(samples, params) {
            return None;
        }
        self.run(cancel, |gpu| gpu.develop(samples, params, cancel))
    }
    pub(crate) fn present(
        &mut self,
        samples: &Arc<super::pipeline::Samples>,
        params: &super::pipeline::pixel_params::PixelParams,
        recipe: &Recipe,
        finish: &gpu::Finish,
        display: &gpu::Display,
        cancel: Cancel<'_>,
    ) -> Option<gpu::Frame> {
        if !self.fits(samples, params) {
            return None;
        }
        self.run(cancel, |gpu| {
            gpu.present(
                gpu::Input::Cpu(samples),
                params,
                recipe,
                finish,
                display,
                cancel,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    fn image(width: u32, height: u32, value: f32) -> CameraImage {
        CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| [value + (i % 7) as f32 * 0.01, value, value * 0.5])
                .collect(),
            metadata: crate::camera_data::Metadata {
                width,
                height,
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
    /// A render in a space other than sRGB never reaches the preview's GPU or caches:
    /// given them, it is refused before anything renders.
    #[test]
    fn only_srgb_renders_take_the_previews_stages() {
        let mut p = PreviewRenderer::default();
        let im = image(40, 30, 0.2);
        let recipe = Recipe::default();
        let r = recipe.checked().unwrap();
        let resolved = quality::resolve(&im, &r);
        for space in [
            crate::color::OutputSpace::AdobeRgb,
            crate::color::OutputSpace::ProPhoto,
        ] {
            let refused = quality::render_preview(
                &im,
                &resolved,
                None,
                0,
                None,
                space,
                Cancel::NEVER,
                Some(&mut p.stages(None)),
            );
            assert!(refused.is_err(), "{space:?}");
        }
        let srgb = quality::render_preview(
            &im,
            &resolved,
            None,
            0,
            None,
            crate::color::OutputSpace::Srgb,
            Cancel::NEVER,
            Some(&mut p.stages(None)),
        );
        assert!(srgb.is_ok());
    }
    #[test]
    fn fit_uses_a_pyramid_level_and_never_reuses_another_photo() {
        let mut p = PreviewRenderer::default();
        let r = Recipe::default();
        let cancel = AtomicBool::new(false);
        let a = image(400, 300, 0.2);
        let fit = p.render(&a, &r, 60, None, Cancel::new(&cancel)).unwrap();
        assert_eq!((fit.width, fit.height), (60, 45));
        let source = p.pyramid.as_ref().unwrap().source().clone();
        assert!(Arc::ptr_eq(&source, a.recovered.get().unwrap()));
        assert_eq!(p.pyramid.as_mut().unwrap().level_for(60.).width, 100);
        let b = image(400, 300, 0.6);
        let other = p.render(&b, &r, 60, None, Cancel::new(&cancel)).unwrap();
        assert_ne!(fit.pixels, other.pixels);
        assert!(!Arc::ptr_eq(p.pyramid.as_ref().unwrap().source(), &source));
        // Regions and full-size views always use the full image.
        let region = p
            .render(&a, &r, 0, Some([0, 0, 10, 10]), Cancel::new(&cancel))
            .unwrap();
        assert_eq!(region.width, 10);
        let full = p.render(&a, &r, 400, None, Cancel::new(&cancel)).unwrap();
        assert_eq!(
            full.pixels,
            quality::render(&a, &r.checked().unwrap(), 400, None, Cancel::NEVER)
                .unwrap()
                .pixels
        );
    }
    /// A textured photo with detail, local and spatial effects: the pyramid Fit stays
    /// close to the export render resized to the same size.
    #[test]
    fn fit_approximates_the_resized_export() {
        let (w, h) = (640, 424);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.25 + 0.2 * (x * 0.05).sin() * (y * 0.031).cos() + 0.05 * (x * 0.9).sin();
            *p = [v * 1.1, v, v * 0.7 + x / w as f32 * 0.2];
        }
        let mut r = Recipe {
            sharpening: 0.8,
            exposure: 0.4,
            straighten: 2.,
            crop: [0.05, 0.1, 0.95, 0.9],
            ..Default::default()
        };
        r.effects.clarity = 0.4;
        r.effects.texture = 0.3;
        r.effects.vignette = -0.4;
        // No grain: the measured grain is noise of the full-resolution photo, whose
        // pixels the Fit and the resized export average differently (its strength at
        // each size is `effects::grain`'s).
        let cancel = AtomicBool::new(false);
        for edge in [100, 180, 300] {
            let expected =
                quality::render(&im, &r.checked().unwrap(), edge, None, Cancel::NEVER).unwrap();
            let fit = PreviewRenderer::default()
                .render(&im, &r, edge, None, Cancel::new(&cancel))
                .unwrap();
            assert_eq!((fit.width, fit.height), (expected.width, expected.height));
            let error = fit
                .pixels
                .iter()
                .flatten()
                .zip(expected.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / (fit.pixels.len() * 3) as f32;
            // 0.0115 at edge 100. The old vignette darkened most of the frame and hid
            // part of the pyramid's error; Camera Raw's mask leaves more of it lit.
            assert!(error < 0.012, "edge {edge}: mean error {error}");
        }
    }
    /// A photo with fine detail, and its half-size decode: each pixel the mean of a
    /// 2 × 2 block, marked `fast` and keeping the photo's metadata, as
    /// `demosaic::half` makes it.
    fn photo_and_half(w: u32, h: u32) -> (CameraImage, CameraImage) {
        let mut full = image(w, h, 0.);
        for (i, p) in full.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.3 + 0.12 * (x * 0.21).sin() * (y * 0.17).cos() + 0.06 * (x * 0.09).sin();
            *p = [v * 1.1, v, v * 0.7];
        }
        let (hw, hh) = (w / 2, h / 2);
        let mut half = full.clone();
        (half.width, half.height, half.fast) = (hw, hh, true);
        half.pixels = (0..hw * hh)
            .map(|i| {
                let (x, y) = (2 * (i % hw), 2 * (i / hw));
                let at = |dx: u32, dy: u32| full.pixels[((y + dy) * w + x + dx) as usize];
                let [a, b, c, d] = [at(0, 0), at(1, 0), at(0, 1), at(1, 1)];
                std::array::from_fn(|k| (a[k] + b[k] + c[k] + d[k]) / 4.)
            })
            .collect();
        (full, half)
    }
    fn mean_error(a: &Rendered, b: &Rendered) -> f32 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        a.pixels
            .iter()
            .flatten()
            .zip(b.pixels.iter().flatten())
            .map(|(x, y)| (x - y).abs())
            .sum::<f32>()
            / (a.pixels.len() * 3) as f32
    }
    /// A half-size decode renders as its photo rendered whole and resized to the same
    /// size: sharpening is sized in the photo's pixels, not the reduced image's, so a
    /// Standard preview built from it, or an editable preview, sharpens as an export does.
    #[test]
    fn a_reduced_decode_sharpens_as_the_photo_resized() {
        let (w, h) = (640, 424);
        let (full, half) = photo_and_half(w, h);
        let recipe = Recipe {
            sharpening: 1.,
            sharpening_radius: 3.,
            ..Default::default()
        };
        let r = recipe.checked().unwrap();
        let expected = quality::render(&full, &r, w / 2, None, Cancel::NEVER).unwrap();
        let reduced = quality::render(&half, &r, w / 2, None, Cancel::NEVER).unwrap();
        let unsharpened = Recipe {
            sharpening: 0.,
            ..Default::default()
        };
        let unsharpened = unsharpened.checked().unwrap();
        let plain = quality::render(&half, &unsharpened, w / 2, None, Cancel::NEVER).unwrap();
        let error = mean_error(&reduced, &expected);
        // What sharpening adds to the photo resized: the error must be a small share of
        // it. Measured: 0.25 of it; sharpened at the full-size radius, 7.1 times it.
        let effect = mean_error(&plain, &expected);
        assert!(
            error < 0.35 * effect,
            "error {error} against sharpening's {effect}"
        );
    }
    /// The Fit of a half-size decode, from its pyramid as the Library's edited previews
    /// render it, sharpens as the photo's Fit does.
    #[test]
    fn a_reduced_decode_s_fit_sharpens_as_the_photo_s() {
        let (w, h) = (640, 424);
        let (full, half) = photo_and_half(w, h);
        let r = Recipe {
            sharpening: 1.,
            sharpening_radius: 3.,
            ..Default::default()
        };
        let unsharpened = Recipe {
            sharpening: 0.,
            ..r.clone()
        };
        let cancel = AtomicBool::new(false);
        let fit = |im: &CameraImage, r: &Recipe| {
            PreviewRenderer::default()
                .render(im, r, 200, None, Cancel::new(&cancel))
                .unwrap()
        };
        let expected = fit(&full, &r);
        let error = mean_error(&fit(&half, &r), &expected);
        let effect = mean_error(&fit(&half, &unsharpened), &expected);
        // Measured: 0.04 of it; sharpened at the full-size radius, 11 times it.
        assert!(
            error < 0.35 * effect,
            "error {error} against sharpening's {effect}"
        );
    }
    /// A half-size decode's grain is as strong as its photo's grain resized to the same
    /// size: drawn for the photo's pixels, not the reduced image's.
    #[test]
    fn a_reduced_decode_grains_as_the_photo_resized() {
        let (w, h) = (640, 424);
        let mut full = image(w, h, 0.);
        full.pixels.fill([0.18; 3]);
        let mut half = image(w / 2, h / 2, 0.);
        half.pixels.fill([0.18; 3]);
        (half.metadata.width, half.metadata.height, half.fast) = (w, h, true);
        let mut r = Recipe {
            sharpening: 0.,
            ..Default::default()
        };
        r.effects.grain = 0.6;
        r.effects.grain_size = 0.25;
        let checked = r.checked().unwrap();
        let spread = |im: &Rendered| {
            let n = im.pixels.len() as f32;
            let mean = im.pixels.iter().map(|p| p[1]).sum::<f32>() / n;
            (im.pixels.iter().map(|p| (p[1] - mean).powi(2)).sum::<f32>() / n).sqrt()
        };
        let expected = quality::render(&full, &checked, w / 2, None, Cancel::NEVER).unwrap();
        let reduced = quality::render(&half, &checked, w / 2, None, Cancel::NEVER).unwrap();
        // Measured: 1.12; drawn for the reduced image's own pixels, 2.4.
        let ratio = spread(&reduced) / spread(&expected);
        assert!((0.8..1.25).contains(&ratio), "grain strength ratio {ratio}");
    }
    /// A full-size decode whose metadata gives another size (a decoder's margins or
    /// turned sensor) renders exactly as before: only a reduced decode is scaled.
    #[test]
    fn a_full_decode_renders_at_its_own_scale_whatever_its_metadata_says() {
        let (full, _) = photo_and_half(320, 212);
        let mut other = full.clone();
        (other.metadata.width, other.metadata.height) = (640, 424);
        let mut r = Recipe {
            sharpening: 1.,
            sharpening_radius: 3.,
            ..Default::default()
        };
        r.effects.grain = 0.5;
        let checked = r.checked().unwrap();
        let r = &checked;
        let a = quality::render(&full, r, 0, None, Cancel::NEVER).unwrap();
        let b = quality::render(&other, r, 0, None, Cancel::NEVER).unwrap();
        assert_eq!(a.pixels, b.pixels);
    }
    fn hash(x: u32, y: u32, c: u32) -> f32 {
        let mut v =
            x.wrapping_mul(0x9e3779b9) ^ y.wrapping_mul(0x85ebca6b) ^ c.wrapping_mul(0xc2b2ae35);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2c1b3c6d);
        v ^= v >> 12;
        (v as f32 / u32::MAX as f32) * 2. - 1.
    }
    /// A noisy photo, as a camera makes one: an RGGB mosaic of a grey flat, a coloured
    /// flat and fine texture side by side, with shot noise whose spread grows with the
    /// square root of the signal, demosaiced bilinearly; and its half-size decode, each
    /// pixel a 2 × 2 block's red, mean green and blue, as `demosaic::half` makes it.
    fn noisy_photo(w: u32, h: u32, amplitude: f32) -> (CameraImage, CameraImage) {
        let colour = |x: u32, y: u32| match (x % 2, y % 2) {
            (0, 0) => 0,
            (1, 1) => 2,
            _ => 1,
        };
        let mosaic: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (fx, fy) = (x as f32, y as f32);
                let clean = match 3 * x / w {
                    0 => [0.18; 3],
                    1 => [0.25, 0.15, 0.08],
                    _ => {
                        let v = 0.25
                            + 0.08 * (fx * 0.6).sin() * (fy * 0.45).cos()
                            + 0.05 * (fx * 0.23 + fy * 0.11).sin();
                        [v * 1.1, v, v * 0.8]
                    }
                };
                let v = clean[colour(x, y)];
                v + amplitude * v.sqrt() * hash(x, y, 0)
            })
            .collect();
        let mut full = image(w, h, 0.);
        full.pixels = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let mut sum = [0.; 3];
                let mut n = [0.; 3];
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                        if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                            continue;
                        }
                        let (xx, yy) = (xx as u32, yy as u32);
                        let c = colour(xx, yy);
                        if c == colour(x, y) && (dx, dy) != (0, 0) {
                            continue;
                        }
                        sum[c] += mosaic[(yy * w + xx) as usize];
                        n[c] += 1.;
                    }
                }
                std::array::from_fn(|c| sum[c] / n[c])
            })
            .collect();
        let (hw, hh) = (w / 2, h / 2);
        let mut half = full.clone();
        (half.width, half.height, half.fast) = (hw, hh, true);
        half.pixels = (0..hw * hh)
            .map(|i| {
                let (x, y) = (2 * (i % hw), 2 * (i / hw));
                let at = |dx: u32, dy: u32| mosaic[((y + dy) * w + x + dx) as usize];
                [at(0, 0), (at(1, 0) + at(0, 1)) / 2., at(1, 1)]
            })
            .collect();
        (full, half)
    }
    /// `full` reduced by area averaging to `w` × `h`, marked `fast` and keeping the
    /// photo's metadata, as a reduced decode is.
    fn reduce(full: &CameraImage, w: u32, h: u32) -> CameraImage {
        let (fw, fh) = (full.width as f32 / w as f32, full.height as f32 / h as f32);
        let mut out = full.clone();
        (out.width, out.height, out.fast) = (w, h, true);
        // Not the photo's highlight recovery, which a render of it may have cached.
        out.recovered = Default::default();
        let weights = |o: u32, f: f32, n: u32| -> Vec<(u32, f32)> {
            let (a, b) = (o as f32 * f, (o + 1) as f32 * f);
            (a.floor() as u32..(b.ceil() as u32).min(n))
                .map(|i| (i, (b.min(i as f32 + 1.) - a.max(i as f32)) / f))
                .filter(|(_, w)| *w > 0.)
                .collect()
        };
        out.pixels = (0..w * h)
            .map(|i| {
                let (ox, oy) = (i % w, i / w);
                let mut s = [0.; 3];
                for (y, wy) in weights(oy, fh, full.height) {
                    for (x, wx) in weights(ox, fw, full.width) {
                        let p = full.pixels[(y * full.width + x) as usize];
                        for c in 0..3 {
                            s[c] += p[c] * wx * wy;
                        }
                    }
                }
                s
            })
            .collect();
        out
    }
    /// The spread of `f` over the flat in columns `x0..x1` (shares of the width, rows
    /// 5–95%) of `im`, after averaging it in `block` × `block` blocks: the noise at
    /// scales of `block` output pixels and coarser.
    fn noise_at(im: &Rendered, x0: f32, x1: f32, block: u32, f: impl Fn([f32; 3]) -> f32) -> f32 {
        let (w, h) = (im.width as f32, im.height as f32);
        let (x0, x1) = ((x0 * w) as u32, (x1 * w) as u32);
        let (y0, y1) = ((0.05 * h) as u32, (0.95 * h) as u32);
        let values: Vec<f32> = (0..(y1 - y0) / block)
            .flat_map(|by| (0..(x1 - x0) / block).map(move |bx| (bx, by)))
            .map(|(bx, by)| {
                let mut sum = 0.;
                for y in 0..block {
                    for x in 0..block {
                        let (x, y) = (x0 + bx * block + x, y0 + by * block + y);
                        sum += f(im.pixels[(y * im.width + x) as usize]);
                    }
                }
                sum / (block * block) as f32
            })
            .collect();
        let n = values.len() as f32;
        let mean = values.iter().sum::<f32>() / n;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n).sqrt()
    }
    /// The share of noise a reduced decode keeps in `small`'s render with `r`, over the
    /// share the photo `full` keeps rendered with `r` and resized to the same size, in
    /// the grey flat's luminance and red − green, the coloured flat's blue − green and
    /// the texture's luminance, at scales of 1, 2, 4 and 8 output pixels; with the mean
    /// error of the two renders. Each share is of the same image's noise without noise
    /// reduction, which the reduction and the resize already change.
    fn noise_kept(full: &CameraImage, small: &CameraImage, r: &Recipe) -> ([[f32; 4]; 4], f32) {
        let r = r.checked().unwrap();
        let mut plain = (*r).clone();
        (plain.noise_luma, plain.noise_chroma) = (0., 0.);
        let plain = plain.checked().unwrap();
        let edge = small.width.max(small.height);
        let render =
            |im: &CameraImage, r| quality::render(im, r, edge, None, Cancel::NEVER).unwrap();
        let (got, expected) = (render(small, &r), render(full, &r));
        let (got_plain, expected_plain) = (render(small, &plain), render(full, &plain));
        type Measure = fn([f32; 3]) -> f32;
        let luma: Measure = |p| p[0] + 2. * p[1] + p[2];
        let regions: [(Measure, f32, f32); 4] = [
            (luma, 0.03, 0.30),
            (|p| p[0] - p[1], 0.03, 0.30),
            (|p| p[2] - p[1], 0.36, 0.63),
            (luma, 0.70, 0.97),
        ];
        let kept = regions.map(|(f, x0, x1)| {
            [1, 2, 4, 8].map(|b| {
                let share = |im, plain| noise_at(im, x0, x1, b, f) / noise_at(plain, x0, x1, b, f);
                share(&got, &got_plain) / share(&expected, &expected_plain)
            })
        });
        (kept, mean_error(&got, &expected))
    }
    /// A reduced decode's luminance and colour noise reduction act on the photo's
    /// scales: each keeps about the share of the noise at each scale that the photo,
    /// denoised and resized, keeps. A half-size decode (from the mosaic, as
    /// `demosaic::half` makes it) and the photo reduced to 0.43 by area, as an
    /// editable preview is, are checked.
    #[test]
    fn a_reduced_decode_reduces_noise_as_the_photo_resized() {
        let (w, h) = (1024, 640);
        let (full, half) = noisy_photo(w, h, 0.06);
        let reduced = reduce(&full, 440, 275);
        // Name, luminance Amount, Detail and Contrast, colour Amount, Detail and
        // Smoothness, and for the half-size decode and the reduced one the worst
        // |ln| of the shares kept, measured, and the mean error, measured. Before
        // their bands were the photo's (in their own pixels, five luminance and five
        // colour levels whatever the scale) the worst |ln| were: 0.02 0.01, 0.56 0.46,
        // 0.22 0.21, 1.02 0.58, 0.47 0.32 and 0.35 0.29, and the mean errors 0.0087
        // 0.0025, 0.0082 0.0022, 0.0081 0.0019, 0.0037 0.0023, 0.0034 0.0021 and
        // 0.0037 0.0025. At scales of 2 output pixels and more, every case is closer
        // than it was but two reduced to 0.43, a little further: colour 10 (0.43
        // against 0.40) and colour 25 with Detail and Smoothness 80 (0.25 against
        // 0.20). The reduced decode's coarsest level straddles the photo's last, and
        // is filtered for the 0.78 of it before the photo's residual (whole, those two
        // were 0.42 and 0.17, and luminance 75 0.10). At 1 pixel the decode also holds
        // noise from the photo's finer scales (neither is filtered before it is
        // reduced), which no threshold of the photo's at its scale takes out as the
        // photo's finest level does. Mean error is mostly the noise left: it falls
        // with any extra reduction, so it is only held, not compared.
        type Case = (&'static str, [f32; 6], [f32; 2], [f32; 2]);
        let cases: [Case; 6] = [
            (
                "luminance 25",
                [0.25, 0.5, 0., 0., 0.5, 0.5],
                [0.02, 0.01],
                [0.0087, 0.0025],
            ),
            (
                "luminance 50, Detail 20, Contrast 50",
                [0.5, 0.2, 0.5, 0., 0.5, 0.5],
                [0.39, 0.34],
                [0.0085, 0.0022],
            ),
            (
                "luminance 75",
                [0.75, 0.5, 0., 0., 0.5, 0.5],
                [0.07, 0.053],
                [0.0081, 0.0019],
            ),
            (
                "colour 10",
                [0., 0.5, 0., 0.1, 0.5, 0.5],
                [0.61, 0.71],
                [0.0049, 0.0025],
            ),
            (
                "colour 25",
                [0., 0.5, 0., 0.25, 0.5, 0.5],
                [0.13, 0.15],
                [0.0034, 0.0020],
            ),
            (
                "colour 25, Detail 80, Smoothness 80",
                [0., 0.5, 0., 0.25, 0.8, 0.8],
                [0.37, 0.25],
                [0.0038, 0.0022],
            ),
        ];
        for (name, [l, ld, lc, c, cd, cs], worst, error) in cases {
            let mut r = Recipe {
                sharpening: 0.,
                noise_luma: l,
                noise_chroma: c,
                ..Default::default()
            };
            r.effects.luma_detail = ld;
            r.effects.luma_contrast = lc;
            r.effects.chroma_detail = cd;
            r.effects.chroma_smoothness = cs;
            for (k, small) in [&half, &reduced].into_iter().enumerate() {
                let (kept, e) = noise_kept(&full, small, &r);
                let off = kept
                    .as_flattened()
                    .iter()
                    .map(|k| k.ln().abs())
                    .fold(0., f32::max);
                // About 1.4 times the measured values, and at least 0.05 for the shares.
                let bound = (worst[k] * 1.4).max(0.05);
                assert!(
                    off < bound,
                    "{name}, {k}: shares kept {kept:.2?}, worst |ln| {off}"
                );
                assert!(e < error[k] * 1.4, "{name}, {k}: mean error {e}");
                // The texture keeps its detail: within 10% of the photo's at every scale.
                assert!(
                    kept[3].iter().all(|k| (k - 1.).abs() < 0.1),
                    "{name}, {k}: texture {kept:.2?}"
                );
            }
        }
    }
    fn mean_difference(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
        let sum: f32 = a
            .iter()
            .flatten()
            .zip(b.iter().flatten())
            .map(|(x, y)| (x - y).abs())
            .sum();
        sum / (a.len() * 3) as f32
    }
    /// A reduced decode's noise reduction changes continuously with its scale: the
    /// half-size decode of a photo 1003 to 1043 pixels wide, in steps of 5 (offsets
    /// 0.97 to 1.03), changes by about as much at every step, including the one
    /// across offset 1, where its coarsest level stops straddling the photo's last.
    /// A level only partly within the photo's filtered levels is filtered in
    /// proportion. Measured: the largest step 1.14 times the median, and 0.055% of
    /// what noise reduction changes. Filtered whole or not at all, the step across
    /// offset 1 was 1.3 to 2.1% of it: 44 and 88 times the median step for colour,
    /// where luminance otherwise does not change at all; snapping the offset to 1
    /// within 0.02 only moved it to the step across 0.98.
    #[test]
    fn a_reduced_decode_s_noise_reduction_changes_continuously_with_its_scale() {
        let (_, half) = noisy_photo(1024, 640, 0.06);
        for (luma, chroma) in [(0.5, 0.), (0., 0.25), (0.6, 0.4)] {
            let r = Recipe {
                noise_luma: luma,
                noise_chroma: chroma,
                ..Default::default()
            };
            let r = r.checked().unwrap();
            let denoised: Vec<_> = [1003, 1008, 1013, 1018, 1023, 1028, 1033, 1038, 1043]
                .map(|width| {
                    let mut im = half.clone();
                    im.metadata.width = width;
                    quality::denoised_camera(&im, &r, None, Cancel::NEVER)
                        .unwrap()
                        .pixels
                        .clone()
                })
                .into();
            let steps: Vec<f32> = denoised
                .windows(2)
                .map(|w| mean_difference(&w[0], &w[1]))
                .collect();
            let effect = mean_difference(&denoised[2], &half.pixels);
            let mut sorted = steps.clone();
            sorted.sort_by(f32::total_cmp);
            let (median, most) = (sorted[sorted.len() / 2], sorted[sorted.len() - 1]);
            // About 2.6 times the measured spread, and as much again where the steps
            // are next to nothing (luminance's, above the finest band).
            assert!(
                most < 3. * median.max(1e-5 * effect),
                "{luma} {chroma}: steps {steps:?}"
            );
            assert!(
                most < 1e-3 * effect,
                "{luma} {chroma}: steps {steps:?} of {effect}"
            );
        }
    }
    /// A half-size decode of a photo with an odd side, a pixel more than half of it
    /// (`demosaic::half` rounds up), denoises almost as one of the even photo.
    /// Measured: 0.03% of what noise reduction changes; filtering the straddling
    /// level whole, as the photo's last, it was 2%.
    #[test]
    fn an_odd_photo_s_half_decode_denoises_as_an_even_one_s() {
        let (_, even) = noisy_photo(512, 320, 0.06);
        let mut odd = even.clone();
        (odd.metadata.width, odd.metadata.height) = (511, 319);
        let r = Recipe {
            noise_luma: 0.5,
            noise_chroma: 0.25,
            ..Default::default()
        };
        let r = r.checked().unwrap();
        let denoised = |im: &CameraImage| {
            quality::denoised_camera(im, &r, None, Cancel::NEVER)
                .unwrap()
                .pixels
                .clone()
        };
        let (a, b) = (denoised(&even), denoised(&odd));
        let (difference, effect) = (mean_difference(&a, &b), mean_difference(&a, &even.pixels));
        assert!(difference < 4e-4 * effect, "{difference} of {effect}");
    }
    /// A full decode whose metadata gives another size denoises as one whose metadata
    /// gives its own: only a reduced decode's bands are the photo's. (That a full
    /// decode denoises bit for bit as before this was checked against the earlier
    /// build when it was made.)
    #[test]
    fn a_full_decode_denoises_at_its_own_scale_whatever_its_metadata_says() {
        let (full, _) = noisy_photo(320, 200, 0.06);
        let mut other = full.clone();
        (other.metadata.width, other.metadata.height) = (640, 400);
        let mut r = Recipe {
            noise_luma: 0.6,
            noise_chroma: 0.4,
            ..Default::default()
        };
        r.effects.luma_detail = 0.3;
        r.effects.luma_contrast = 0.4;
        let checked = r.checked().unwrap();
        let a = quality::render(&full, &checked, 0, None, Cancel::NEVER).unwrap();
        let b = quality::render(&other, &checked, 0, None, Cancel::NEVER).unwrap();
        assert_eq!(a.pixels, b.pixels);
    }
    /// Every edit, including ones that change only cached stages, must render exactly
    /// as a renderer without cached state would.
    #[test]
    fn cached_stages_never_serve_a_stale_result() {
        let (w, h) = (300, 200);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.3 + 0.2 * (x * 0.07).sin() * (y * 0.05).cos();
            *p = [v * 1.2, v, v * 0.6];
        }
        let cancel = AtomicBool::new(false);
        let mut warm = PreviewRenderer::default();
        let mut r = Recipe {
            shadows: 0.3,
            ..Default::default()
        };
        r.effects.clarity = 0.3;
        let edits: [&dyn Fn(&mut Recipe); 20] = [
            &|_| {},
            &|r| r.exposure = 0.5,
            &|r| r.effects.clarity = -0.2,
            &|r| r.effects.texture = 0.4,
            &|r| r.wb[0] = 1.3,
            &|r| r.crop = [0.1, 0., 0.9, 1.],
            &|r| r.noise_luma = 0.4,
            &|r| r.effects.luma_detail = 0.9,
            &|r| r.effects.luma_contrast = 0.7,
            &|r| r.noise_luma = 0.,
            &|r| r.noise_luma = 0.6,
            &|r| r.contrast = 0.3,
            &|r| r.whites = 0.6,
            &|r| r.blacks = -0.4,
            &|r| r.highlights = -0.5,
            &|r| r.effects.dehaze = 0.4,
            &|r| r.effects.calibration[2] = [0.2, -0.1],
            &|r| r.profile_amount = 0.5,
            &|r| {
                use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
                r.masks.push(MaskGroup {
                    components: vec![MaskComponent::new(MaskShape::Linear {
                        from: [0.4, 0.5],
                        to: [0.6, 0.5],
                    })],
                    adjust: LocalAdjust {
                        exposure: 0.5,
                        shadows: 0.3,
                        ..Default::default()
                    },
                    ..Default::default()
                });
            },
            &|r| r.masks[0].adjust.whites = 0.4,
        ];
        for edit in edits {
            edit(&mut r);
            for (edge, region) in [(80, None), (0, Some([20, 30, 50, 40])), (150, None)] {
                let cached = warm
                    .render(&im, &r, edge, region, Cancel::new(&cancel))
                    .unwrap();
                let fresh = PreviewRenderer::default()
                    .render(&im, &r, edge, region, Cancel::new(&cancel))
                    .unwrap();
                assert_eq!(cached.pixels, fresh.pixels, "{r:?} {edge} {region:?}");
            }
        }
        // The stage cache filled up to its entry limit across the edits.
        assert_eq!(warm.cache.samples.len(), 4);
    }
    /// The scene tone stage's measures are kept per measurement copy and the settings
    /// they read: white balance, temperature, the profile and calibration measure the
    /// photo again; Exposure and the tone sliders reuse the measures, and Dehaze reuses
    /// the haze measured with them. Cached renders match fresh ones either way.
    #[test]
    fn photo_measures_are_kept_until_their_settings_change() {
        let (w, h) = (300, 200);
        let mut im = image(w, h, 0.);
        im.metadata.cam_xyz = [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.52],
        ];
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.02 + 0.6 * (x / w as f32) * (0.6 + 0.4 * (y * 0.05).cos());
            *p = [v * 1.3, v, v * 0.5 + 0.05 * (x * 0.1).sin().abs()];
        }
        // Whites, Blacks and Dehaze follow the measures, so stale ones would show.
        let mut base = Recipe {
            profile: Some(Arc::new(
                crate::camera_profiles::CameraProfile::creative_for_test(&im.metadata),
            )),
            whites: 0.5,
            blacks: -0.4,
            ..Default::default()
        };
        base.effects.dehaze = 0.3;
        let metadata = im.metadata.clone();
        // (edit, measures again, the measures change)
        type Case<'a> = (&'a str, &'a dyn Fn(&mut Recipe), bool, bool);
        let cases: [Case; 11] = [
            ("exposure", &|r| r.exposure = 0.7, false, false),
            ("more dehaze", &|r| r.effects.dehaze = 0.8, false, false),
            ("whites", &|r| r.whites = 0.8, false, false),
            ("shadows", &|r| r.shadows = 0.4, false, false),
            ("blacks", &|r| r.blacks = -0.7, false, false),
            ("dehaze", &|r| r.effects.dehaze = -0.3, false, false),
            // A mask's positive Dehaze adds to the recipe's and reads the same haze.
            (
                "mask dehaze",
                &|r| {
                    let mut mask = crate::model::masks::MaskGroup {
                        components: vec![crate::model::masks::MaskComponent::new(
                            crate::model::masks::MaskShape::Linear {
                                from: [0.2, 0.5],
                                to: [0.8, 0.5],
                            },
                        )],
                        ..Default::default()
                    };
                    mask.adjust.dehaze = 0.5;
                    r.masks.push(mask);
                },
                false,
                false,
            ),
            ("white balance", &|r| r.wb = [1.4, 1., 0.7], true, true),
            // As the Temperature slider sets it: with the white balance it maps to.
            (
                "temperature",
                &|r| {
                    r.temperature = 3500.;
                    r.update_wb(&metadata);
                },
                true,
                true,
            ),
            (
                "calibration",
                &|r| r.effects.calibration[0] = [0.5, 0.4],
                true,
                true,
            ),
            // The look's strength leaves the measures, but the key keeps the profile
            // whole: measured again.
            ("profile amount", &|r| r.profile_amount = 0.5, true, false),
        ];
        let cancel = AtomicBool::new(false);
        for (name, edit, again, changed) in cases {
            let mut warm = PreviewRenderer::default();
            let mut r = base.clone();
            let check = |warm: &mut PreviewRenderer, r: &Recipe| {
                for (edge, region) in [(80, None), (0, Some([20, 30, 50, 40])), (150, None)] {
                    let cached = warm
                        .render(&im, r, edge, region, Cancel::new(&cancel))
                        .unwrap();
                    let fresh = PreviewRenderer::default()
                        .render(&im, r, edge, region, Cancel::new(&cancel))
                        .unwrap();
                    assert_eq!(cached.pixels, fresh.pixels, "{name} {edge} {region:?}");
                }
            };
            check(&mut warm, &r);
            // Fit and 100% regions share one measure.
            assert_eq!(warm.cache.measures.lock().unwrap().len(), 1, "{name}");
            let rendered_haze = warm
                .cache
                .measures
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .haze
                .get()
                .cloned();
            edit(&mut r);
            check(&mut warm, &r);
            let measures = warm.cache.measures.lock().unwrap();
            assert_eq!(measures.len(), 1 + again as usize, "{name}");
            // The haze is measured with the measures it was kept with, once.
            let first = measures.values().last().unwrap();
            let haze = first.haze.get().expect("positive Dehaze measures the haze");
            if !again {
                assert!(
                    Arc::ptr_eq(haze, rendered_haze.as_ref().unwrap()),
                    "{name}: measured the haze again"
                );
            }
            let values: Vec<_> = measures.values().map(|m| m.measures).collect();
            assert_eq!(
                values.first() != values.last(),
                changed,
                "{name}: {values:?}"
            );
        }
    }
    /// The Contrast pivot is kept with the measures by Dehaze, and the
    /// Shadows/Highlights map's base by Exposure: the Basic sliders, Clarity and masks
    /// reuse them, and every cached render matches a fresh one.
    #[test]
    fn the_pivot_and_map_are_kept_until_their_settings_change() {
        let (w, h) = (300, 200);
        let mut im = image(w, h, 0.);
        im.metadata.cam_xyz = [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.52],
        ];
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.02 + 0.6 * (x / w as f32) * (0.6 + 0.4 * (y * 0.05).cos());
            *p = [v * 1.3, v, v * 0.5 + 0.05 * (x * 0.1).sin().abs()];
        }
        let mut base = Recipe {
            profile: Some(Arc::new(
                crate::camera_profiles::CameraProfile::creative_for_test(&im.metadata),
            )),
            contrast: 0.4,
            shadows: 0.3,
            highlights: -0.4,
            ..Default::default()
        };
        base.effects.dehaze = 0.3;
        // (edit, kept pivots and map bases after it)
        type Case<'a> = (&'a str, &'a dyn Fn(&mut Recipe), [usize; 2]);
        let cases: [Case; 9] = [
            ("contrast", &|r| r.contrast = -0.6, [1, 1]),
            ("shadows", &|r| r.shadows = 0.7, [1, 1]),
            ("negative shadows", &|r| r.shadows = -0.5, [1, 1]),
            ("highlights", &|r| r.highlights = 0.2, [1, 1]),
            ("whites", &|r| r.whites = 0.5, [1, 1]),
            ("clarity", &|r| r.effects.clarity = 0.4, [1, 1]),
            (
                "mask shadows",
                &|r| {
                    let mut mask = crate::model::masks::MaskGroup {
                        components: vec![crate::model::masks::MaskComponent::new(
                            crate::model::masks::MaskShape::Linear {
                                from: [0.2, 0.5],
                                to: [0.8, 0.5],
                            },
                        )],
                        ..Default::default()
                    };
                    mask.adjust.shadows = 0.5;
                    mask.adjust.clarity = -0.3;
                    r.masks.push(mask);
                },
                [1, 1],
            ),
            ("exposure", &|r| r.exposure = 0.6, [1, 2]),
            ("dehaze", &|r| r.effects.dehaze = 0.7, [2, 1]),
        ];
        let cancel = AtomicBool::new(false);
        for (name, edit, kept) in cases {
            let mut warm = PreviewRenderer::default();
            let mut r = base.clone();
            let check = |warm: &mut PreviewRenderer, r: &Recipe| {
                for (edge, region) in [(80, None), (0, Some([20, 30, 50, 40])), (150, None)] {
                    let cached = warm
                        .render(&im, r, edge, region, Cancel::new(&cancel))
                        .unwrap();
                    let fresh = PreviewRenderer::default()
                        .render(&im, r, edge, region, Cancel::new(&cancel))
                        .unwrap();
                    assert_eq!(cached.pixels, fresh.pixels, "{name} {edge} {region:?}");
                }
            };
            check(&mut warm, &r);
            edit(&mut r);
            check(&mut warm, &r);
            let measures = warm.cache.measures.lock().unwrap();
            assert_eq!(measures.len(), 1, "{name}");
            let measured = measures.values().next().unwrap();
            assert_eq!(measured.kept(), kept, "{name}");
        }
    }
    #[test]
    fn mask_shadows_reduce_the_photo_once() {
        use crate::model::masks::{MaskComponent, MaskGroup, MaskShape};
        let (w, h) = (300, 200);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let v = 0.1 + 0.3 * (i as u32 % w) as f32 / w as f32;
            *p = [v * 1.2, v, v * 0.6];
        }
        let cancel = AtomicBool::new(false);
        let mut warm = PreviewRenderer::default();
        let mut r = Recipe {
            ..Default::default()
        };
        let mut mask = MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Linear {
                from: [0.2, 0.5],
                to: [0.8, 0.5],
            })],
            ..Default::default()
        };
        mask.adjust.shadows = 0.5;
        r.masks.push(mask);
        for exposure in [0., 0.3] {
            r.exposure = exposure;
            let cached = warm
                .render(&im, &r, 150, None, Cancel::new(&cancel))
                .unwrap();
            let fresh = PreviewRenderer::default()
                .render(&im, &r, 150, None, Cancel::new(&cancel))
                .unwrap();
            assert_eq!(cached.pixels, fresh.pixels);
        }
        // Exposure comes after the map's input: one reduction serves both renders.
        assert_eq!(warm.cache.measured.len(), 1);
    }
    /// Spot removal renders the same in Fit, 100% regions and exports, and edits to
    /// it patch the cached pyramid correctly.
    #[test]
    fn retouch_agrees_between_fit_regions_and_export() {
        use crate::model::retouch::{RetouchMode, RetouchOp, RetouchShape};
        let (w, h) = (640, 424);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.25 + 0.15 * (x * 0.05).sin() * (y * 0.031).cos() + x / w as f32 * 0.2;
            let dust = (x - 300.).hypot(y - 200.) < 8.;
            *p = if dust {
                [0.02; 3]
            } else {
                [v * 1.1, v, v * 0.7]
            };
        }
        let spot = |mode, center: [f32; 2], offset: [f32; 2]| RetouchOp {
            mode,
            shape: RetouchShape::Spot {
                center,
                radius: 15. / 640.,
            },
            feather: 0.4,
            opacity: 1.,
            offset,
        };
        let mut r = Recipe {
            sharpening: 0.5,
            straighten: 1.5,
            ..Default::default()
        };
        r.retouch = vec![
            spot(
                RetouchMode::Heal,
                [300.5 / 640., 200.5 / 424.],
                [0.08, 0.02],
            ),
            spot(RetouchMode::Clone, [0.2, 0.7], [0.1, -0.1]),
        ];
        let cancel = AtomicBool::new(false);
        let mut warm = PreviewRenderer::default();
        for edit in 0..3 {
            match edit {
                1 => r.retouch[1].offset[0] += 0.05,
                2 => {
                    r.retouch.remove(0);
                }
                _ => {}
            }
            let expected =
                quality::render(&im, &r.checked().unwrap(), 200, None, Cancel::NEVER).unwrap();
            let fit = warm
                .render(&im, &r, 200, None, Cancel::new(&cancel))
                .unwrap();
            let fresh = PreviewRenderer::default()
                .render(&im, &r, 200, None, Cancel::new(&cancel))
                .unwrap();
            assert_eq!(fit.pixels, fresh.pixels, "edit {edit}: stale pyramid");
            let error = fit
                .pixels
                .iter()
                .flatten()
                .zip(expected.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / (fit.pixels.len() * 3) as f32;
            assert!(error < 0.01, "edit {edit}: mean error {error}");
            let region = [260, 160, 90, 80];
            let tile = warm
                .render(&im, &r, 0, Some(region), Cancel::new(&cancel))
                .unwrap();
            let full = quality::render(&im, &r.checked().unwrap(), 0, Some(region), Cancel::NEVER)
                .unwrap();
            assert_eq!(tile.pixels, full.pixels);
        }
        // The healed dust is gone from the export.
        r.retouch = vec![spot(
            RetouchMode::Heal,
            [300.5 / 640., 200.5 / 424.],
            [0.08, 0.02],
        )];
        r.straighten = 0.;
        let healed = quality::render(
            &im,
            &r.checked().unwrap(),
            0,
            Some([300, 200, 1, 1]),
            Cancel::NEVER,
        )
        .unwrap();
        let dusty = quality::render(
            &im,
            &Recipe::default().checked().unwrap(),
            0,
            Some([300, 200, 1, 1]),
            Cancel::NEVER,
        )
        .unwrap();
        assert!(healed.pixels[0][1] > dusty.pixels[0][1] + 0.2);
    }
    /// A local repair does not move the whole photo's darkest level
    /// (`PhotoMeasures::dark`, which the default black follows): a red eye correction
    /// darkening a pupil and spot removal healing a dark speck leave it as without
    /// them, in Fit and 100% regions alike, while the minimum (Blacks') sees them.
    #[test]
    fn retouching_leaves_the_photos_darkest_level() {
        use crate::model::{
            red_eye::RedEyeOp,
            retouch::{RetouchMode, RetouchOp, RetouchShape},
        };
        let (w, h) = (480, 320);
        let spot = [300., 120.];
        // A spot of `color` 12 pixels across at `spot` (0.3% of the photo, more than
        // the percentile) on a lighter background.
        let photo = |color: [f32; 3]| {
            let mut im = image(w, h, 0.);
            for (i, p) in im.pixels.iter_mut().enumerate() {
                let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
                *p = if (x - spot[0]).hypot(y - spot[1]) < 12. {
                    color
                } else {
                    [0.5, 0.33, 0.25]
                };
            }
            im
        };
        let cancel = AtomicBool::new(false);
        let measures = |im: &CameraImage, r: &Recipe| {
            let mut warm = PreviewRenderer::default();
            warm.render(im, r, 150, None, Cancel::new(&cancel)).unwrap();
            warm.render(im, r, 0, Some([40, 30, 60, 50]), Cancel::new(&cancel))
                .unwrap();
            let cached = warm.cache.measures.lock().unwrap();
            assert_eq!(cached.len(), 1, "Fit and regions share the measures");
            cached.values().next().unwrap().measures
        };
        let frame = crate::model::image_frame::ImageFrame::new(&image(w, h, 0.));
        let eye = photo([0.6, 0.03, 0.03]);
        let fixed = Recipe {
            red_eye: vec![RedEyeOp {
                kind: Default::default(),
                center: frame.to_image(spot[0], spot[1]),
                radius: [12. / 480.; 2],
                correlation: 0.,
                pupil_size: 0.5,
                darken: 1.,
            }]
            .into(),
            ..Default::default()
        };
        let speck = photo([0.02; 3]);
        let healed = Recipe {
            retouch: vec![RetouchOp {
                mode: RetouchMode::Heal,
                shape: RetouchShape::Spot {
                    center: frame.to_image(spot[0], spot[1]),
                    radius: 20. / 480.,
                },
                feather: 0.4,
                opacity: 1.,
                offset: [-0.2, 0.2],
            }],
            ..Default::default()
        };
        for (name, im, r) in [("red eye", &eye, &fixed), ("spot", &speck, &healed)] {
            let (plain, retouched) = (measures(im, &Recipe::default()), measures(im, r));
            assert!(
                (retouched.min - plain.min).abs() > 0.5,
                "{name}: the repair changes the photo ({} {})",
                retouched.min,
                plain.min
            );
            assert_eq!(retouched.dark, plain.dark, "{name}");
        }
    }
    /// A red eye correction stays on its eye through crop, straightening, rotation and
    /// flips, and renders the same in Fit, regions and exports.
    #[test]
    fn red_eye_follows_geometry_and_agrees_between_previews_and_export() {
        use crate::{develop::ViewMapping, model::red_eye::RedEyeOp};
        let (w, h) = (480, 320);
        let mut im = image(w, h, 0.);
        let eye = [300., 120.];
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            *p = if (x - eye[0]).hypot(y - eye[1]) < 12. {
                [0.6, 0.03, 0.03]
            } else {
                [0.5, 0.33, 0.25]
            };
        }
        let frame = crate::model::image_frame::ImageFrame::new(&im);
        let op = RedEyeOp {
            kind: Default::default(),
            center: frame.to_image(eye[0], eye[1]),
            radius: [12. / 480.; 2],
            correlation: 0.,
            pupil_size: 0.5,
            darken: 0.5,
        };
        let cancel = AtomicBool::new(false);
        let mut warm = PreviewRenderer::default();
        for (rotation, flip_x, straighten, crop) in [
            (0, false, 0., [0., 0., 1., 1.]),
            (1, false, 4., [0.3, 0.1, 0.95, 0.8]),
            (3, true, -6., [0.4, 0., 1., 0.7]),
        ] {
            let mut r = Recipe {
                rotation,
                flip_x,
                straighten,
                crop,
                ..Default::default()
            };
            let at = |r: &Recipe| {
                let full =
                    quality::render(&im, &r.checked().unwrap(), 0, None, Cancel::NEVER).unwrap();
                let [u, v] = ViewMapping::new(&im, r).to_view(op.center);
                let (x, y) = (
                    (u * full.width as f32) as usize,
                    (v * full.height as f32) as usize,
                );
                full.pixels[y * full.width as usize + x]
            };
            let red = at(&r);
            r.red_eye = vec![op.clone()].into();
            let fixed = at(&r);
            assert!(red[0] > 3. * red[1], "{rotation}: red before {red:?}");
            assert!(
                fixed[0] < 1.3 * fixed[1] && fixed[0] < 0.5 * red[0],
                "{rotation}: {fixed:?} from {red:?}"
            );
            let fit = warm
                .render(&im, &r, 150, None, Cancel::new(&cancel))
                .unwrap();
            let fresh = PreviewRenderer::default()
                .render(&im, &r, 150, None, Cancel::new(&cancel))
                .unwrap();
            assert_eq!(fit.pixels, fresh.pixels, "{rotation}: stale pyramid");
            let region = [40, 30, 60, 50];
            let tile = warm
                .render(&im, &r, 0, Some(region), Cancel::new(&cancel))
                .unwrap();
            let full = quality::render(&im, &r.checked().unwrap(), 0, Some(region), Cancel::NEVER)
                .unwrap();
            assert_eq!(tile.pixels, full.pixels);
        }
    }
    /// Mask edits (sliders, shapes, ranges, visibility) never reuse stale weights.
    #[test]
    fn cached_mask_weights_follow_every_edit() {
        use crate::model::masks::{LocalAdjust, MaskComponent, MaskGroup, MaskShape};
        let (w, h) = (300, 200);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.3 + 0.2 * (x * 0.07).sin() * (y * 0.05).cos();
            *p = [v * 1.2, v, v * 0.6];
        }
        let cancel = AtomicBool::new(false);
        let mut warm = PreviewRenderer::default();
        let mut r = Recipe::default();
        r.masks.push(MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Linear {
                from: [0.2, 0.5],
                to: [0.6, 0.5],
            })],
            adjust: LocalAdjust {
                exposure: 0.7,
                ..Default::default()
            },
            ..Default::default()
        });
        let edits: [&dyn Fn(&mut Recipe); 7] = [
            &|_| {},
            &|r| r.masks[0].adjust.exposure = -0.5,
            &|r| r.masks[0].adjust.contrast = 0.4,
            &|r| {
                r.masks[0].components[0].shape = MaskShape::Radial {
                    center: [0.5, 0.5],
                    radii: [0.2, 0.1],
                    angle: 20.,
                    feather: 0.3,
                }
            },
            &|r| {
                r.masks[0].components.push(MaskComponent {
                    op: crate::model::masks::MaskOp::Intersect,
                    ..MaskComponent::new(MaskShape::LuminanceRange {
                        low: 0.3,
                        high: 1.,
                        falloff: [0.1, 0.],
                    })
                })
            },
            &|r| r.exposure = 0.4,
            &|r| r.masks[0].hidden = true,
        ];
        for edit in edits {
            edit(&mut r);
            for (edge, region) in [(80, None), (0, Some([20, 30, 50, 40]))] {
                let cached = warm
                    .render(&im, &r, edge, region, Cancel::new(&cancel))
                    .unwrap();
                let fresh = PreviewRenderer::default()
                    .render(&im, &r, edge, region, Cancel::new(&cancel))
                    .unwrap();
                assert_eq!(cached.pixels, fresh.pixels, "{:?} {edge}", r.masks);
            }
        }
    }
    #[test]
    fn region_preview_is_a_half_resolution_region() {
        let (w, h) = (400, 300);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.3 + 0.2 * (x * 0.04).sin() * (y * 0.03).cos();
            *p = [v * 1.2, v, v * 0.6];
        }
        let cancel = AtomicBool::new(false);
        let mut r = Recipe::default();
        r.effects.clarity = 0.3;
        let mut p = PreviewRenderer::default();
        let region = [101, 80, 120, 90];
        let preview = p
            .render_region_preview(&im, &r, region, Cancel::new(&cancel))
            .unwrap();
        assert_eq!((preview.width, preview.height), (60, 45));
        let full = p
            .render(&im, &r, 0, Some(region), Cancel::new(&cancel))
            .unwrap();
        let mut error = 0.;
        for y in 0..45 {
            for x in 0..60 {
                let a = preview.pixels[y * 60 + x];
                let b = full.pixels[2 * y * 120 + 2 * x];
                error += (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>();
            }
        }
        let error = error / (60. * 45. * 3.);
        assert!(error < 0.02, "mean error {error}");
    }
    #[test]
    #[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
    fn a_region_beyond_the_gpu_limits_renders_on_the_cpu_and_keeps_the_gpu() {
        use crate::develop::pipeline::{Samples, pixel_params::PixelParams};
        let mut p = PreviewRenderer::with_gpu();
        assert!(p.gpu().is_some(), "{:?}", p.fallback_reason());
        let cancel = AtomicBool::new(false);
        let params = PixelParams {
            params: Vec::new(),
            tables: Vec::new(),
            weights: Vec::new(),
            grid: None,
            color: None,
        };
        // Over the develop memory budget, then over the device's buffer limits; their
        // size alone, as the region is refused before its pixels are read.
        for edge in [5_000, 40_000] {
            let samples = Arc::new(Samples {
                width: edge,
                height: edge,
                pixels: Vec::new(),
                positions: Vec::new(),
            });
            assert!(
                p.backend
                    .develop(&samples, &params, Cancel::new(&cancel))
                    .is_none()
            );
            assert!(p.gpu().is_some(), "{edge}: {:?}", p.fallback_reason());
        }
        // The next, smaller render still uses the GPU (finishing, at full size).
        p.render(
            &image(400, 300, 0.2),
            &Recipe::default(),
            0,
            None,
            Cancel::new(&cancel),
        )
        .unwrap();
        assert!(p.used_gpu());
    }
}
