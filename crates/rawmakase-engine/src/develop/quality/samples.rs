//! What the panels read from the developed photo: highlights recovered once, retouching applied, and the Point Color and Targeted Adjustment samples.
use super::*;
use crate::develop::recipe::RenderedRecipe;

/// The highlight-recovered image, computed once per decoded image.
pub fn recovered(im: &CameraImage, cancel: &AtomicBool) -> Result<Arc<CameraImage>> {
    if let Some(recovered) = im.recovered.get() {
        return Ok(recovered.clone());
    }
    let recovered = Arc::new(recover_highlights_cancellable(im, cancel)?);
    Ok(im.recovered.get_or_init(|| recovered).clone())
}
/// The recovered image with the measured Color noise reduction, then the recipe's red
/// eye corrections and spot removal, so those stay within their shapes: from the
/// preview's cache, updated where the operations changed, or built at once (exports).
pub(crate) fn retouched(
    im: &CameraImage,
    r: &Recipe,
    cancel: &AtomicBool,
    cache: Option<&mut develop::retouch::RetouchCache>,
) -> Result<Arc<CameraImage>> {
    let recovered = recovered(im, cancel)?;
    let ops = develop::retouch::Retouching::of(r);
    let denoise = r.chroma_denoise();
    match cache {
        Some(cache) => {
            let base = cache.denoised(&recovered, denoise, cancel)?;
            cache.get(&base, ops, cancel)
        }
        None => {
            let base = match denoise {
                Some(d) => Arc::new(d.apply(&recovered, cancel)?),
                None => recovered,
            };
            Ok(match ops.is_empty() {
                true => base,
                false => Arc::new(develop::retouch::apply(&base, ops)),
            })
        }
    }
}
/// Point Color's dropper at (`u`, `v`) of the shown photo: the color Point Color sees
/// there, averaged over 5×5 output pixels, rendered as the photo is (lens corrections,
/// retouching, masks and the swatches already there included), as a swatch's
/// `source`: HSV of linear ProPhoto RGB with the hue in sixths of a turn.
pub fn point_color_pick(
    im: &CameraImage,
    r: &Recipe,
    u: f32,
    v: f32,
    cancel: &AtomicBool,
) -> Result<[f32; 3]> {
    let [mean] = stage_means(
        im,
        r,
        u,
        v,
        [develop::pipeline::PixelOutput::PointColor],
        cancel,
    )?;
    let [h, s, v] = crate::color::hsv::rgb_to_hsv(mean);
    Ok([
        (h / std::f32::consts::TAU * 6.).rem_euclid(6.),
        s.clamp(0., 1.),
        v.clamp(0., 1.),
    ])
}
/// The Targeted Adjustment Tool at (`u`, `v`) of the shown photo: what the tone
/// curve, the color mixer and the black & white mix see there, averaged over 5×5
/// output pixels and rendered as the photo is.
pub fn targeted_sample(
    im: &CameraImage,
    r: &Recipe,
    u: f32,
    v: f32,
    cancel: &AtomicBool,
) -> Result<develop::targeted::TargetSample> {
    use develop::pipeline::PixelOutput;
    let [tone, mixer, color] = stage_means(
        im,
        r,
        u,
        v,
        [
            PixelOutput::CurveInput,
            PixelOutput::MixerInput,
            PixelOutput::ColorInput,
        ],
        cancel,
    )?;
    Ok(develop::targeted::TargetSample {
        tone: tone[0],
        mixer,
        color,
    })
}
/// A stage between the per-pixel pipeline's stages (docs/scene-tone-stage.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Linear ProPhoto RGB entering the scene tone stage.
    SceneInput,
    /// Linear ProPhoto RGB leaving it, before the profile's look and tone curve: where
    /// Camera Raw's renders of a linear-profile DNG compare.
    SceneOutput,
    /// The encoded colour Auto tone measures (`PixelOutput::AutoBasis`).
    AutoBasis,
}
/// The whole photo at full size, as linear ProPhoto RGB at `stage`, through the same
/// sampling, lens correction, retouching and masks as the render: for comparing a
/// stage with Camera Raw.
pub fn render_stage(
    im: &CameraImage,
    r: &Recipe,
    stage: Stage,
    cancel: &AtomicBool,
) -> Result<crate::rendered::Rendered> {
    use develop::pipeline::PixelOutput;
    let shown = r.as_rendered();
    shown.validate()?;
    let effective = shown.resolved(&im.metadata);
    let r = effective.as_ref();
    if r.lens_ca {
        crate::lens::auto_ca::prime(im);
    }
    let source = retouched(im, r, cancel, None)?;
    let g = Geometry::new(&source, r, 0);
    let (toned, tonal) = local_stage(&source, &source, r, 1., cancel, None)?;
    let output = match stage {
        Stage::SceneInput => PixelOutput::SceneInput,
        Stage::SceneOutput => PixelOutput::SceneOutput,
        Stage::AutoBasis => PixelOutput::AutoBasis,
    };
    develop::pipeline::stage_samples(
        &toned,
        &tonal,
        &g,
        [0, 0, g.width, g.height],
        output,
        cancel,
    )
}
/// The mean of 5×5 output pixels around (`u`, `v`) at each of `outputs`' stages.
fn stage_means<const N: usize>(
    im: &CameraImage,
    r: &Recipe,
    u: f32,
    v: f32,
    outputs: [develop::pipeline::PixelOutput; N],
    cancel: &AtomicBool,
) -> Result<[[f32; 3]; N]> {
    let shown = r.as_rendered();
    shown.validate()?;
    let effective = shown.resolved(&im.metadata);
    let r = effective.as_ref();
    if r.lens_ca {
        crate::lens::auto_ca::prime(im);
    }
    // Through a fresh retouch cache, which checks `cancel` between operations.
    let mut retouch = develop::retouch::RetouchCache::default();
    let source = retouched(im, r, cancel, Some(&mut retouch))?;
    let g = Geometry::new(&source, r, 0);
    let (toned, tonal) = local_stage(&source, &source, r, 1., cancel, None)?;
    let at = |t: f32, size: u32| {
        let c = (t.clamp(0., 1.) * size as f32) as u32;
        c.saturating_sub(2).min(size.saturating_sub(5))
    };
    let region = [
        at(u, g.width),
        at(v, g.height),
        g.width.min(5),
        g.height.min(5),
    ];
    let mut means = [[0.; 3]; N];
    for (mean, output) in means.iter_mut().zip(outputs) {
        use develop::pipeline::PixelOutput;
        let out = develop::pipeline::stage_samples(&toned, &tonal, &g, region, output, cancel)?;
        let n = out.pixels.len() as f32;
        // ProPhoto RGB without negative channels, as Point Color and the mixer see it.
        let floor = if matches!(output, PixelOutput::PointColor | PixelOutput::MixerInput) {
            0.
        } else {
            f32::NEG_INFINITY
        };
        *mean =
            std::array::from_fn(|c| out.pixels.iter().map(|p| p[c].max(floor)).sum::<f32>() / n);
    }
    Ok(means)
}
