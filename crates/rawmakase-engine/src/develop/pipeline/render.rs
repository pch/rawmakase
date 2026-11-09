//! The render entry points, from a whole photo to a region, and the samples the panels read.
use super::*;
use rawmakase_cancel::Cancel;

/// Shared full/preview renderer. Geometry is sampled in rows; no full-sized intermediate color image.
/// Stops with an error once `cancel` is set.
pub fn render(
    im: &CameraImage,
    r: &ValidRecipe,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<Rendered> {
    crate::develop::quality::render(im, r, max_edge, None, cancel)
}
/// [`render`](fn@render) of a photo with AI Denoise: `ai` is its source, resolved by
/// the caller (`None` renders without AI Denoise whatever the recipe says, as a preview
/// does before a result is ready; exports must not pass `None` for a recipe that has it).
pub fn render_with_ai(
    im: &CameraImage,
    r: &ValidRecipe,
    ai: Option<&crate::develop::AiSource>,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<Rendered> {
    crate::develop::quality::render_with_ai(im, r, ai, max_edge, cancel)
}
/// [`render_with_ai`] for an export in `space`, on the CPU: the colour stage ends in
/// `space` and every finishing stage works on its values.
pub fn render_export(
    im: &CameraImage,
    r: &ValidRecipe,
    ai: Option<&crate::develop::AiSource>,
    max_edge: u32,
    space: crate::color::OutputSpace,
    cancel: Cancel<'_>,
) -> Result<crate::develop::ExportImage> {
    crate::develop::quality::render_export(im, r, ai, max_edge, space, cancel)
}
pub fn render_region(im: &CameraImage, r: &ValidRecipe, region: [u32; 4]) -> Result<Rendered> {
    crate::develop::quality::render(im, r, 0, Some(region), Cancel::NEVER)
}
/// Unsharpened render of `region` of the output described by `g`, and the mask weights
/// the finishing stages need. A `spread` above zero averages each sample over a
/// footprint (see [`footprint_spread`]).
///
/// With a `cache`, the geometry, lens-warp and noise-reduction samples are kept, so a
/// following render that only changes color and tone reruns the per-pixel stage alone.
/// A `backend` with a GPU runs that stage there when the port covers the recipe.
///
/// The pixels are encoded in `space`. Only sRGB renders may have `stages`: the GPU
/// and its caches are the preview's, and an export in another space never reaches
/// them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_base(
    toned: &Toned,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: Cancel<'_>,
    stages: Option<&mut crate::develop::preview_renderer::Stages>,
    space: crate::color::OutputSpace,
) -> Result<(Rendered, Option<Arc<MaskWeights>>)> {
    ensure!(
        space == crate::color::OutputSpace::Srgb || stages.is_none(),
        "Only sRGB renders run on the GPU"
    );
    let mut base = r.clone();
    base.sharpening = 0.;
    let im = toned.source();
    let masked = base
        .masks
        .iter()
        .any(crate::model::masks::MaskGroup::is_active);
    let Some(stages) = stages else {
        if !masked {
            return Ok((
                render_region_inner(im, &base, g, region, spread, cancel, space)?,
                None,
            ));
        }
        base.validate()?;
        let samples = Arc::new(sample_region(im, &base, g, region, spread, cancel)?);
        let weights = mask_weights(
            toned,
            &base,
            g,
            region,
            spread,
            Some(&samples),
            None,
            cancel,
        )?;
        let samples = detail(toned, &base, samples, weights.as_deref(), None, cancel)?;
        let out = develop_samples_in(im, &base, &samples, cancel, weights.as_deref(), space)?;
        return Ok((out, weights));
    };
    base.validate()?;
    let key = crate::develop::stage_cache::SampleKey::new(toned, &base, g, region, spread);
    let samples = stages.cache.samples.get_or_try(key, Samples::bytes, || {
        sample_region(im, &base, g, region, spread, cancel)
    })?;
    let weights = mask_weights(
        toned,
        &base,
        g,
        region,
        spread,
        Some(&samples),
        Some(&mut *stages),
        cancel,
    )?;
    let samples = detail(
        toned,
        &base,
        samples,
        weights.as_deref(),
        Some(&mut *stages.cache),
        cancel,
    )?;
    let backend = &mut *stages.backend;
    if backend.has_gpu()
        && let Some(mut params) = pixel_params::pixel_params(im, &base)
        && params.set_masks(im, &base, weights.as_deref())
        && let Some(out) = backend.develop(&samples, &params, cancel)
    {
        return Ok((out, weights));
    }
    let out = develop_samples(im, &base, &samples, cancel, weights.as_deref())?;
    Ok((out, weights))
}
/// As [`render_base`] followed by sharpening, spatial effects and display, all on the
/// GPU into a texture for the stages' display. `None` without a GPU or display, or when
/// the port does not cover `r` (the tonal recipe); `finished` is the full recipe.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_display(
    toned: &Toned,
    r: &Recipe,
    finished: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    finish: &crate::develop::gpu::Finish,
    cancel: Cancel<'_>,
    stages: &mut crate::develop::preview_renderer::Stages,
) -> Result<Option<crate::develop::gpu::Frame>> {
    let Some(display) = stages.display else {
        return Ok(None);
    };
    if !stages.backend.has_gpu() {
        return Ok(None);
    }
    let mut base = r.clone();
    base.sharpening = 0.;
    base.validate()?;
    let im = toned.source();
    let Some(mut params) = pixel_params::pixel_params(im, &base) else {
        return Ok(None);
    };
    let key = crate::develop::stage_cache::SampleKey::new(toned, &base, g, region, spread);
    let samples = stages.cache.samples.get_or_try(key, Samples::bytes, || {
        sample_region(im, &base, g, region, spread, cancel)
    })?;
    let weights = mask_weights(
        toned,
        &base,
        g,
        region,
        spread,
        Some(&samples),
        Some(&mut *stages),
        cancel,
    )?;
    // Local Sharpness and Noise finish on the CPU.
    if weights
        .as_ref()
        .is_some_and(|w| w.uses(&[slot::SHARPNESS, slot::NOISE]))
        || !params.set_masks(im, &base, weights.as_deref())
    {
        return Ok(None);
    }
    let samples = detail(
        toned,
        &base,
        samples,
        weights.as_deref(),
        Some(&mut *stages.cache),
        cancel,
    )?;
    Ok(stages
        .backend
        .present(&samples, &params, finished, finish, display, cancel))
}
/// Weights of the recipe's active masks over `region` of `g`, rendered from `toned`'s
/// image, through the stage cache when there is one. Range components first develop
/// `samples` without local adjustments; without samples they cannot be evaluated and
/// the result is `None`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mask_weights(
    toned: &Toned,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    samples: Option<&Arc<Samples>>,
    stages: Option<&mut crate::develop::preview_renderer::Stages>,
    cancel: Cancel<'_>,
) -> Result<Option<Arc<MaskWeights>>> {
    use crate::develop::masks::{Selection, Weigher};
    if !r
        .masks
        .iter()
        .any(crate::model::masks::MaskGroup::is_active)
    {
        return Ok(None);
    }
    // A mask whose raster cannot be provided is an error, never a mask that selects
    // nothing: the edit would otherwise be shown or exported without it.
    for m in r.masks.iter().filter(|m| m.is_active()) {
        rawmakase_model::storage::mask_assets::ensure_shaped(crate::model::masks::bitmap_refs(
            std::slice::from_ref(m),
        ))?;
    }
    let ranges = r
        .masks
        .iter()
        .any(|m| m.is_active() && m.components.iter().any(|c| c.shape.is_range()));
    let key = crate::develop::stage_cache::MaskKey::new(toned, r, g, region, spread, ranges);
    let mut stages = stages;
    if let Some(s) = stages.as_deref_mut()
        && let Some(hit) = s.cache.masks.get(&key)
    {
        return Ok(Some(Arc::new(hit.with_deltas(&r.masks))));
    }
    let weigher = Weigher::cached(
        &toned.image,
        &r.masks,
        Selection::Active,
        stages.as_deref_mut().map(|s| &mut s.cache.rasters),
    );
    let global = if weigher.needs_range() {
        let Some(samples) = samples else {
            return Ok(None);
        };
        let mut plain = r.clone();
        plain.masks.clear();
        // Range masks select from the photo as it renders, never as Visualize Range
        // grays it, and always in sRGB, whatever space an export is in: a mask
        // selects the same pixels in every space.
        crate::model::point_color::without_visualization(&mut plain.point_colors);
        let im = toned.source();
        let gpu = stages.as_deref_mut().and_then(|s| {
            let params = pixel_params::pixel_params(im, &plain)?;
            s.backend.develop(samples, &params, cancel)
        });
        Some(match gpu {
            Some(out) => out,
            None => develop_samples(im, &plain, samples, cancel, None)?,
        })
    } else {
        None
    };
    ensure!(!cancel.is_cancelled(), "Render superseded");
    let weights = Arc::new(weigher.weights(&toned.image, r, g, region, global.as_ref()));
    if let Some(s) = stages {
        s.cache.masks.insert(key, weights.clone(), weights.bytes());
    }
    Ok(Some(weights))
}
/// A mask's Texture: the samples scaled by the measured Texture's detail of the camera
/// image at their positions (`texture.rs`), by the mask's strength on top of the
/// global slider's, which the image already has. A mask's Clarity renders in the scene
/// tone stage.
pub(super) fn detail(
    toned: &Toned,
    r: &Recipe,
    samples: Arc<Samples>,
    weights: Option<&MaskWeights>,
    cache: Option<&mut crate::develop::stage_cache::StageCache>,
    cancel: Cancel<'_>,
) -> Result<Arc<Samples>> {
    let Some(weights) = weights.filter(|w| w.uses(&[slot::TEXTURE])) else {
        return Ok(samples);
    };
    let source = toned.untextured.as_ref().unwrap_or(&toned.image);
    let texture = crate::develop::quality::texture_detail(source, toned.scale, cancel, cache)?;
    let global = crate::develop::texture::strength(r.effects.texture);
    let (w, h) = (source.width as usize, source.height as usize);
    let mut out = Samples {
        width: samples.width,
        height: samples.height,
        pixels: samples.pixels.clone(),
        positions: samples.positions.clone(),
    };
    out.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        let Some(d) = weights.delta(i) else {
            return;
        };
        let [x, y] = samples.positions[i];
        if d[slot::TEXTURE] == 0. || x.is_nan() {
            return;
        }
        let s = crate::develop::texture::strength(r.effects.texture + d[slot::TEXTURE]) - global;
        let detail = texture.at(x, y, w, h);
        for (v, d) in p.iter_mut().zip(detail) {
            *v *= (s * d).exp2();
        }
    });
    Ok(Arc::new(out))
}
/// Camera samples of an output region after geometry, lens correction and noise
/// reduction, with their source positions; `NAN` positions lie outside the photo.
pub(crate) struct Samples {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<[f32; 3]>,
    pub(crate) positions: Vec<[f32; 2]>,
}
impl Samples {
    pub(super) fn bytes(&self) -> usize {
        self.pixels.len() * 20
    }
}
pub(crate) fn sample_region(
    im: Source,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: Cancel<'_>,
) -> Result<Samples> {
    let [x0, y0, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x0.checked_add(w).is_some_and(|r| r <= g.width)
            && y0.checked_add(h).is_some_and(|b| b <= g.height),
        "Invalid viewport region"
    );
    let warp = LensWarp::new(&im, r);
    let (pixels, positions) = (0..w as usize * h as usize)
        .into_par_iter()
        .map(|i| {
            if cancel.is_cancelled() {
                return ([0.; 3], [f32::NAN; 2]);
            }
            let x = x0 + i as u32 % w;
            let y = y0 + i as u32 / w;
            let [sx, sy] = g.source(
                (x as f32 + 0.5) / g.width as f32,
                (y as f32 + 0.5) / g.height as f32,
            );
            if g.outside(sx, sy) {
                return ([1.; 3], [f32::NAN; 2]);
            }
            let (p, pos) = match &warp {
                Some(w) => (w.sample(im, sx, sy, spread), w.map.forward(sx, sy)),
                None => (footprint_sample(im, sx, sy, spread), [sx, sy]),
            };
            // Where the file has no image (a panorama's empty corners), white,
            // as outside the photo.
            if im.empty_at(pos[0], pos[1]) {
                return ([1.; 3], [f32::NAN; 2]);
            }
            (p, pos)
        })
        .unzip();
    ensure!(!cancel.is_cancelled(), "Render superseded");
    Ok(Samples {
        width: w,
        height: h,
        pixels,
        positions,
    })
}
/// The per-pixel color and tone stage over prepared samples, with the masks'
/// adjustments where `weights` has them.
pub(crate) fn develop_samples(
    im: Source,
    r: &Recipe,
    samples: &Samples,
    cancel: Cancel<'_>,
    weights: Option<&MaskWeights>,
) -> Result<Rendered> {
    develop_samples_in(
        im,
        r,
        samples,
        cancel,
        weights,
        crate::color::OutputSpace::Srgb,
    )
}
/// [`develop_samples`] encoded in `space`.
pub(crate) fn develop_samples_in(
    im: Source,
    r: &Recipe,
    samples: &Samples,
    cancel: Cancel<'_>,
    weights: Option<&MaskWeights>,
    space: crate::color::OutputSpace,
) -> Result<Rendered> {
    develop_samples_to(im, r, samples, cancel, weights, PixelOutput::Display, space)
}
/// `region` of the output as a dropper samples it at the stage `output` names,
/// through the same sampling, lens correction, retouching and masks as the render.
pub(crate) fn stage_samples(
    toned: &Toned,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    output: PixelOutput,
    cancel: Cancel<'_>,
) -> Result<Rendered> {
    let im = toned.source();
    let samples = Arc::new(sample_region(im, r, g, region, 0., cancel)?);
    let weights = mask_weights(toned, r, g, region, 0., Some(&samples), None, cancel)?;
    let samples = detail(toned, r, samples, weights.as_deref(), None, cancel)?;
    develop_samples_to(
        im,
        r,
        &samples,
        cancel,
        weights.as_deref(),
        output,
        crate::color::OutputSpace::Srgb,
    )
}
/// The camera's own samples of `region`, before highlight reconstruction, noise
/// reduction and retouching, through the same geometry, lens correction and
/// default crop as the render, times the recipe's white balance (`Recipe::wb`):
/// white-balanced camera RGB before any profile, matrix or exposure. A pixel
/// outside the photo is NaN.
pub(crate) fn camera_samples(
    im: &CameraImage,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    cancel: Cancel<'_>,
) -> Result<Rendered> {
    let samples = sample_region(Source::new(im), r, g, region, 0., cancel)?;
    let pixels = samples
        .pixels
        .iter()
        .zip(&samples.positions)
        .map(|(p, pos)| match pos[0].is_nan() {
            true => [f32::NAN; 3],
            false => std::array::from_fn(|c| p[c] * r.wb[c]),
        })
        .collect();
    Ok(Rendered {
        width: samples.width,
        height: samples.height,
        pixels,
    })
}
#[allow(clippy::too_many_arguments)]
fn develop_samples_to(
    im: Source,
    r: &Recipe,
    samples: &Samples,
    cancel: Cancel<'_>,
    weights: Option<&MaskWeights>,
    output: PixelOutput,
    space: crate::color::OutputSpace,
) -> Result<Rendered> {
    let camera = camera_stage(&im.metadata, r);
    let local_tone = weights.is_some_and(|w| w.uses(&[slot::SHADOWS, slot::HIGHLIGHTS]));
    let mut lut = CurveSet::for_image(im, r, camera, local_tone);
    lut.output = output;
    lut.space = space;
    let math = weights.map(|_| LocalMath::new(&im.metadata, r));
    let mut pixels = vec![[0.; 3]; samples.pixels.len()];
    pixels.par_iter_mut().enumerate().for_each(|(i, out)| {
        if cancel.is_cancelled() {
            return;
        }
        let pos = samples.positions[i];
        let delta = weights.and_then(|w| w.delta(i));
        let local = delta
            .as_ref()
            .zip(math.as_ref())
            .map(|(delta, math)| Local { delta, math });
        *out = if pos[0].is_nan() {
            [1.; 3]
        } else {
            process_pixel(samples.pixels[i], r, &lut, camera, pos, local)
        };
    });
    ensure!(!cancel.is_cancelled(), "Render superseded");
    Ok(Rendered {
        width: samples.width,
        height: samples.height,
        pixels,
    })
}
/// One sample per output pixel of `region`, without the stage cache: the per-pixel
/// stage over the lens-warped or noise-reduced samples, encoded in `space`.
pub(super) fn render_region_inner(
    im: Source,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: Cancel<'_>,
    space: crate::color::OutputSpace,
) -> Result<Rendered> {
    r.validate()?;
    let camera = camera_stage(&im.metadata, r);
    let mut lut = CurveSet::for_image(im, r, camera, false);
    lut.space = space;
    let [x0, y0, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x0.checked_add(w).is_some_and(|r| r <= g.width)
            && y0.checked_add(h).is_some_and(|b| b <= g.height),
        "Invalid viewport region"
    );
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    let warp = LensWarp::new(&im, r);
    let at = |x: u32, y: u32| {
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        if g.outside(sx, sy) {
            return [1.; 3];
        }
        let (p, pos) = match &warp {
            Some(w) => (w.sample(im, sx, sy, spread), w.map.forward(sx, sy)),
            None => (footprint_sample(im, sx, sy, spread), [sx, sy]),
        };
        if im.empty_at(pos[0], pos[1]) {
            return [1.; 3];
        }
        process_pixel(p, r, &lut, camera, pos, None)
    };
    pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.is_cancelled() {
            return;
        }
        let x = x0 + i as u32 % w;
        let y = y0 + i as u32 / w;
        *p = at(x, y);
        if r.sharpening > 0. {
            let mut avg = [0.; 3];
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let q = at(
                        (x as i64 + dx).clamp(0, g.width as i64 - 1) as u32,
                        (y as i64 + dy).clamp(0, g.height as i64 - 1) as u32,
                    );
                    for c in 0..3 {
                        avg[c] += q[c] / 9.;
                    }
                }
            }
            for c in 0..3 {
                p[c] = (p[c] + (p[c] - avg[c]) * r.sharpening).clamp(0., 1.);
            }
        }
    });
    ensure!(!cancel.is_cancelled(), "Render superseded");
    Ok(Rendered {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod sampling_tests {
    use super::*;

    fn lens_image() -> CameraImage {
        use crate::optics::{LensCorrection, Radial};
        let mut im = CameraImage {
            width: 256,
            height: 128,
            pixels: vec![],
            metadata: Metadata {
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            hdr: Default::default(),
        };
        im.metadata.width = im.width;
        im.metadata.height = im.height;
        im.pixels = (0..im.width * im.height)
            .map(|i| [i as f32 % im.width as f32 / im.width as f32; 3])
            .collect();
        im.metadata.lens = Some(LensCorrection {
            source: "test".into(),
            default_on: true,
            distortion: Some(Radial {
                knots: vec![0., 1.],
                values: vec![1., 1.15],
            }),
            vignetting: None,
            chromatic: None,
            gain_map: None,
            ..Default::default()
        });
        im
    }

    /// Local tone maps are measured in decoded-image coordinates. A corrected sample
    /// must carry the coordinate it actually read, not its position before lens warp.
    #[test]
    fn lens_samples_keep_their_decoded_positions() {
        let im = lens_image();
        let r = Recipe::for_metadata(&im.metadata);
        let g = Geometry::new(&im, &r, 128);
        let samples = sample_region(
            (&im).into(),
            &r,
            &g,
            [0, 0, g.width, g.height],
            0.,
            Cancel::NEVER,
        )
        .unwrap();
        for (p, pos) in samples.pixels.iter().zip(&samples.positions) {
            let expected = pos[0].clamp(0., (im.width - 1) as f32) / im.width as f32;
            assert!(
                (p[1] - expected).abs() < 1e-5,
                "sample {} disagrees with decoded position {:?}: {}",
                p[1],
                pos,
                expected
            );
        }
    }
    #[test]
    fn lens_uncached_render_matches_preview() {
        let im = lens_image();
        let mut r = Recipe::for_metadata(&im.metadata);
        r.profile = Some(Arc::new(
            crate::camera_profiles::CameraProfile::camera_matrix_default(&im.metadata).unwrap(),
        ));
        r.effects.clarity = 1.;
        r.shadows = 0.5;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let expected = crate::develop::PreviewRenderer::default()
            .render(&im, &r, 0, None, Cancel::new(&cancel))
            .unwrap();
        let actual = render(&im, &r.checked().unwrap(), 0, Cancel::NEVER).unwrap();
        let max = actual
            .pixels
            .iter()
            .zip(&expected.pixels)
            .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs()))
            .fold(0f32, f32::max);
        assert!(max < 1e-5, "uncached/preview difference {max}");
    }
}
