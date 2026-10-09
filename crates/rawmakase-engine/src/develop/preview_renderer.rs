//! Stateful desktop preview backend. Export remains on the reference CPU path.
use super::{
    Geometry, gpu,
    pyramid::Pyramid,
    quality::{self, Output},
    stage_cache::StageCache,
};
use crate::rendered::Rendered;
use crate::{
    camera_data::CameraImage,
    model::{recipe::Recipe, valid::ValidRecipe},
};
use anyhow::Result;
use std::sync::{Arc, atomic::AtomicBool};

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
        cancel: &AtomicBool,
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
        cancel: &AtomicBool,
        display: Option<&gpu::Display>,
    ) -> Result<Output> {
        let shown = recipe.as_rendered();
        let recipe = shown.as_ref();
        self.backend.used_gpu = false;
        anyhow::ensure!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "Render superseded"
        );
        let recipe = &recipe.checked()?;
        if region.is_none()
            && max_edge > 0
            && let Some(out) = self.render_fit(image, recipe, max_edge, cancel, display)?
        {
            return Ok(out);
        }
        quality::render_preview(
            image,
            recipe,
            max_edge,
            region,
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
        recipe: &ValidRecipe,
        max_edge: u32,
        cancel: &AtomicBool,
        display: Option<&gpu::Display>,
    ) -> Result<Option<Output>> {
        let full = Geometry::new(image, recipe, 0);
        let long = full.width.max(full.height);
        if long <= max_edge {
            return Ok(None);
        }
        let size = quality::output_size(full.width, full.height, max_edge);
        let needed = size.0.max(size.1) as f32 * image.width.max(image.height) as f32 / long as f32;
        let (level, source) = self.level(image, recipe, needed, cancel)?;
        let region = [0, 0, size.0, size.1];
        let mut stages = self.stages(display);
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
        cancel: &AtomicBool,
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
        cancel: &AtomicBool,
        display: Option<&gpu::Display>,
    ) -> Result<Output> {
        let shown = recipe.as_rendered();
        let recipe = shown.as_ref();
        self.backend.used_gpu = false;
        let recipe = &recipe.checked()?;
        let full = Geometry::new(image, recipe, 0);
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
        let (level, source) = self.level(image, recipe, needed, cancel)?;
        let region = [px, py, pw, ph];
        let mut stages = self.stages(display);
        quality::render_level(&level, &source, recipe, size, region, cancel, &mut stages)
    }
    /// The pyramid level for `needed` source pixels on the long edge, and the level-0
    /// image (recovered and retouched), building the pyramid when the photo changed and
    /// patching it where spot removal changed.
    fn level(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        needed: f32,
        cancel: &AtomicBool,
    ) -> Result<(Arc<CameraImage>, Arc<CameraImage>)> {
        if recipe.lens_ca {
            crate::lens::auto_ca::prime(image);
        }
        let source = quality::retouched(image, recipe, cancel, Some(&mut self.retouch))?;
        match &mut self.pyramid {
            Some(p) if Arc::ptr_eq(p.source(), &source) => {}
            Some(p) if self.retouch.changed_from(p.source()).is_some() => {
                let rects = self.retouch.changed_from(p.source()).unwrap().to_vec();
                p.update(source, &rects);
            }
            _ => self.pyramid = Some(Pyramid::new(source)),
        }
        let pyramid = self.pyramid.as_mut().unwrap();
        Ok((pyramid.level_for(needed), pyramid.source().clone()))
    }
    #[cfg(test)]
    pub(crate) fn finish(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        max_edge: u32,
        cancel: &AtomicBool,
    ) -> Option<Rendered> {
        self.backend.finish(image, recipe, max_edge, cancel)
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
        cancel: &AtomicBool,
        stage: impl FnOnce(&mut gpu::Processor) -> Result<T>,
    ) -> Option<T> {
        if self.resident_fallback.is_some() {
            return None;
        }
        let gpu = self.gpu.as_mut()?;
        match stage(gpu) {
            Ok(out) => Some(out),
            Err(error) => {
                if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    self.resident_fallback = Some(error.to_string());
                }
                None
            }
        }
    }
    pub(crate) fn run<T>(
        &mut self,
        cancel: &AtomicBool,
        stage: impl FnOnce(&mut gpu::Processor) -> Result<T>,
    ) -> Option<T> {
        let gpu = self.gpu.as_mut()?;
        match stage(gpu) {
            Ok(out) => {
                self.used_gpu = true;
                Some(out)
            }
            Err(error) => {
                if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
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
        max_edge: u32,
        cancel: &AtomicBool,
    ) -> Option<Rendered> {
        self.run(cancel, |gpu| gpu.finish(image, recipe, max_edge, cancel))
    }
    pub(crate) fn develop(
        &mut self,
        samples: &Arc<super::pipeline::Samples>,
        params: &super::pipeline::pixel_params::PixelParams,
        cancel: &AtomicBool,
    ) -> Option<Rendered> {
        self.run(cancel, |gpu| gpu.develop(samples, params, cancel))
    }
    pub(crate) fn present(
        &mut self,
        samples: &Arc<super::pipeline::Samples>,
        params: &super::pipeline::pixel_params::PixelParams,
        recipe: &Recipe,
        finish: &gpu::Finish,
        display: &gpu::Display,
        cancel: &AtomicBool,
    ) -> Option<gpu::Frame> {
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
        }
    }
    #[test]
    fn fit_uses_a_pyramid_level_and_never_reuses_another_photo() {
        let mut p = PreviewRenderer::default();
        let r = Recipe::default();
        let cancel = AtomicBool::new(false);
        let a = image(400, 300, 0.2);
        let fit = p.render(&a, &r, 60, None, &cancel).unwrap();
        assert_eq!((fit.width, fit.height), (60, 45));
        let source = p.pyramid.as_ref().unwrap().source().clone();
        assert!(Arc::ptr_eq(&source, a.recovered.get().unwrap()));
        assert_eq!(p.pyramid.as_mut().unwrap().level_for(60.).width, 100);
        let b = image(400, 300, 0.6);
        let other = p.render(&b, &r, 60, None, &cancel).unwrap();
        assert_ne!(fit.pixels, other.pixels);
        assert!(!Arc::ptr_eq(p.pyramid.as_ref().unwrap().source(), &source));
        // Regions and full-size views always use the full image.
        let region = p.render(&a, &r, 0, Some([0, 0, 10, 10]), &cancel).unwrap();
        assert_eq!(region.width, 10);
        let full = p.render(&a, &r, 400, None, &cancel).unwrap();
        assert_eq!(
            full.pixels,
            quality::render(&a, &r.checked().unwrap(), 400, None)
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
            let expected = quality::render(&im, &r.checked().unwrap(), edge, None).unwrap();
            let fit = PreviewRenderer::default()
                .render(&im, &r, edge, None, &cancel)
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
        let edits: [&dyn Fn(&mut Recipe); 16] = [
            &|_| {},
            &|r| r.exposure = 0.5,
            &|r| r.effects.clarity = -0.2,
            &|r| r.effects.texture = 0.4,
            &|r| r.wb[0] = 1.3,
            &|r| r.crop = [0.1, 0., 0.9, 1.],
            &|r| r.noise_luma = 0.4,
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
                let cached = warm.render(&im, &r, edge, region, &cancel).unwrap();
                let fresh = PreviewRenderer::default()
                    .render(&im, &r, edge, region, &cancel)
                    .unwrap();
                assert_eq!(cached.pixels, fresh.pixels, "{r:?} {edge} {region:?}");
            }
        }
        // The stage cache filled up to its entry limit across the edits.
        assert_eq!(warm.cache.samples.len(), 4);
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
            let cached = warm.render(&im, &r, 150, None, &cancel).unwrap();
            let fresh = PreviewRenderer::default()
                .render(&im, &r, 150, None, &cancel)
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
            let expected = quality::render(&im, &r.checked().unwrap(), 200, None).unwrap();
            let fit = warm.render(&im, &r, 200, None, &cancel).unwrap();
            let fresh = PreviewRenderer::default()
                .render(&im, &r, 200, None, &cancel)
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
            let tile = warm.render(&im, &r, 0, Some(region), &cancel).unwrap();
            let full = quality::render(&im, &r.checked().unwrap(), 0, Some(region)).unwrap();
            assert_eq!(tile.pixels, full.pixels);
        }
        // The healed dust is gone from the export.
        r.retouch = vec![spot(
            RetouchMode::Heal,
            [300.5 / 640., 200.5 / 424.],
            [0.08, 0.02],
        )];
        r.straighten = 0.;
        let healed =
            quality::render(&im, &r.checked().unwrap(), 0, Some([300, 200, 1, 1])).unwrap();
        let dusty = quality::render(
            &im,
            &Recipe::default().checked().unwrap(),
            0,
            Some([300, 200, 1, 1]),
        )
        .unwrap();
        assert!(healed.pixels[0][1] > dusty.pixels[0][1] + 0.2);
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
                let full = quality::render(&im, &r.checked().unwrap(), 0, None).unwrap();
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
            let fit = warm.render(&im, &r, 150, None, &cancel).unwrap();
            let fresh = PreviewRenderer::default()
                .render(&im, &r, 150, None, &cancel)
                .unwrap();
            assert_eq!(fit.pixels, fresh.pixels, "{rotation}: stale pyramid");
            let region = [40, 30, 60, 50];
            let tile = warm.render(&im, &r, 0, Some(region), &cancel).unwrap();
            let full = quality::render(&im, &r.checked().unwrap(), 0, Some(region)).unwrap();
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
                let cached = warm.render(&im, &r, edge, region, &cancel).unwrap();
                let fresh = PreviewRenderer::default()
                    .render(&im, &r, edge, region, &cancel)
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
        let preview = p.render_region_preview(&im, &r, region, &cancel).unwrap();
        assert_eq!((preview.width, preview.height), (60, 45));
        let full = p.render(&im, &r, 0, Some(region), &cancel).unwrap();
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
}
