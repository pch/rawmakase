//! What the panels read from the developed photo: highlights recovered once, retouching applied, and the Point Color and Targeted Adjustment samples.
use super::*;
use crate::develop::recipe::RenderedRecipe;
use rawmakase_cancel::Cancel;

/// The highlight-recovered image, computed once per decoded image. A rendered file's
/// is the file as it is: recovery rebuilds a camera's clipped channels, and a rendered
/// file's bright colours are its rendering, not clipping. (That costs one copy of its
/// pixels, as recovery does.)
pub fn recovered(im: &CameraImage, cancel: Cancel<'_>) -> Result<Arc<CameraImage>> {
    if let Some(recovered) = im.recovered.get() {
        return Ok(recovered.clone());
    }
    let recovered = Arc::new(match im.metadata.is_rendered() {
        true => im.clone(),
        false => recover_highlights(im, cancel)?,
    });
    Ok(im.recovered.get_or_init(|| recovered).clone())
}
/// The camera image after highlight recovery, AI Denoise's blend and Color and
/// luminance noise reduction, before retouching: what noise reduction made of the photo,
/// for measuring it (`rawmakase denoise --camera-dump`).
pub fn denoised_camera(
    im: &CameraImage,
    r: &Recipe,
    ai: Option<&develop::AiSource>,
    cancel: Cancel<'_>,
) -> Result<Arc<CameraImage>> {
    Ok(retouched(im, r, ai, cancel, None)?.unretouched)
}
/// The retouched photo at full resolution, and the image it was retouched from.
#[derive(Clone)]
pub(crate) struct Retouched {
    /// Recovered, with Color and luminance noise reduction, red eye corrections and spot removal.
    pub(crate) image: Arc<CameraImage>,
    /// `image` without red eye corrections and spot removal, which the photo's darkest
    /// level is measured on (`PhotoMeasures::dark`): a local repair does not move the
    /// whole photo's black. The same image when the recipe has none.
    pub(crate) unretouched: Arc<CameraImage>,
}
/// The recovered image with Color and luminance noise reduction, then the recipe's red
/// eye corrections and spot removal, so those stay within their shapes: from the
/// preview's cache, updated where the operations changed, or built at once (exports).
pub(crate) fn retouched(
    im: &CameraImage,
    r: &Recipe,
    ai: Option<&develop::AiSource>,
    cancel: Cancel<'_>,
    cache: Option<&mut develop::retouch::RetouchCache>,
) -> Result<Retouched> {
    let recovered = recovered(im, cancel)?;
    let ops = develop::retouch::Retouching::of(r);
    let denoise = r.denoise_with(ai.filter(|a| a.fits(im)));
    let (base, image) = match cache {
        Some(cache) => {
            let base = cache.denoised(&recovered, denoise, cancel)?;
            let image = cache.get(&base, ops, cancel)?;
            (base, image)
        }
        None => {
            let base = match denoise {
                Some(d) => d.apply(&recovered, cancel)?,
                None => recovered,
            };
            let image = match ops.is_empty() {
                true => base.clone(),
                false => Arc::new(develop::retouch::apply(&base, ops)),
            };
            (base, image)
        }
    };
    Ok(Retouched {
        image,
        unretouched: base,
    })
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
    cancel: Cancel<'_>,
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
    cancel: Cancel<'_>,
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
    /// White-balanced camera RGB as the decoder made it, before highlight
    /// reconstruction, noise reduction and retouching, and before the profile, its
    /// matrix and the camera's baseline exposure: the sample times `Recipe::wb`.
    /// Divided by that and the photo's as-shot multipliers (`Metadata::wb`) it is
    /// the channel's fraction of the sensor's white. Pixels outside the photo are
    /// NaN.
    CameraInput,
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
    cancel: Cancel<'_>,
) -> Result<crate::rendered::Rendered> {
    render_stage_sized(im, r, stage, 0, cancel)
}
/// [`render_stage`] at most `max_edge` pixels on the long edge (0 for full size),
/// sampled from the full-size photo as a preview is.
pub fn render_stage_sized(
    im: &CameraImage,
    r: &Recipe,
    stage: Stage,
    max_edge: u32,
    cancel: Cancel<'_>,
) -> Result<crate::rendered::Rendered> {
    use develop::pipeline::PixelOutput;
    let shown = r.as_rendered();
    shown.validate()?;
    let effective = shown.resolved(&im.metadata);
    let r = effective.as_ref();
    if r.lens_ca {
        crate::lens::auto_ca::prime(im);
    }
    if stage == Stage::CameraInput {
        let g = Geometry::new(im, r, max_edge);
        return develop::pipeline::camera_samples(im, r, &g, [0, 0, g.width, g.height], cancel);
    }
    let source = retouched(im, r, None, cancel, None)?;
    let g = Geometry::new(&source.image, r, max_edge);
    let (toned, tonal) = local_stage(&source.image, &source, r, 1., cancel, None)?;
    let output = match stage {
        Stage::SceneInput => PixelOutput::SceneInput,
        Stage::SceneOutput => PixelOutput::SceneOutput,
        Stage::AutoBasis => PixelOutput::AutoBasis,
        Stage::CameraInput => unreachable!("sampled above"),
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
    cancel: Cancel<'_>,
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
    let source = retouched(im, r, None, cancel, Some(&mut retouch))?;
    let g = Geometry::new(&source.image, r, 0);
    let (toned, tonal) = local_stage(&source.image, &source, r, 1., cancel, None)?;
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
