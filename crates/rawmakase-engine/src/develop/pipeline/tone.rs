//! Tone preparation: the curve tables a render looks up, and what it measures from the photo first (the scene tone stage's measures, the contrast pivot).
use super::*;

pub(super) struct CurveSet {
    /// Where the stage stops.
    pub(super) output: PixelOutput,
    pub(super) exposure_gain: f32,
    /// Contrast as a measured Camera Raw curve.
    pub(super) basic: Option<crate::develop::basic_tone::BasicTone>,
    /// The Shadows/Highlights base level, built per image by `for_image`.
    pub(super) local: Option<crate::develop::local_tone::LocalToneMap>,
    /// The measured color mixer, Saturation and Vibrance.
    pub(super) mixer: Option<crate::develop::color_mixer::ColorMixer>,
    /// The black & white mix's grid (`black_white::gray_grid`), for black & white
    /// renders.
    pub(crate) gray_grid: Option<Vec<[f32; 3]>>,
    /// Point Color swatches.
    pub(super) point_colors: Option<crate::develop::point_color::PointColors>,
    /// The measured color grading, when grading is active.
    pub(super) grade: Option<crate::develop::color_grade::ColorGrade>,
    pub(super) color_adjustments: bool,
    /// The profile look's RGB table, at the recipe's Profile Amount.
    pub(super) rgb_table: Option<crate::camera_profiles::RgbLook>,
    pub(super) calibration: crate::develop::calibration::Calibration,
    /// What Contrast follows of the photo, for the global slider and the masks'.
    pub(super) photo: crate::develop::basic_tone::PhotoTone,
    /// The measured parametric curve, when a region is set.
    pub(super) parametric: Option<crate::develop::parametric::ParametricCurve>,
    pub(super) master: CurveLut,
    pub(super) channels: [CurveLut; 3],
    /// What the scene tone stage measured of the photo.
    pub(crate) measures: crate::develop::scene_tone::PhotoMeasures,
    /// The scene tone stage (S3) for these measures.
    pub(super) scene: crate::develop::scene_tone::SceneTone,
}
impl CurveSet {
    /// Curves plus the Shadows/Highlights map of this image (which also carries the
    /// measured Clarity); built also when `local_tone` (masks change Shadows or
    /// Highlights).
    pub(super) fn for_image(
        im: Source,
        r: &Recipe,
        matrix: [[f32; 3]; 3],
        local_tone: bool,
    ) -> Self {
        let mut lut = Self::with_photo_measures(im, r, matrix);
        let small = measured_copy(im);
        lut.local = crate::develop::local_tone::LocalToneMap::build(
            &small,
            [im.width, im.height],
            crate::develop::local_tone::Sliders::of(r),
            local_tone,
            |p| {
                crate::develop::scene_tone::luminance(exposure_stage(
                    scene_color(p, r, &lut, matrix, None),
                    &lut,
                    None,
                ))
            },
        );
        lut
    }
    /// Curves with the scene tone stage for this image's measures, and Contrast at
    /// its pivot when the recipe measures it.
    pub(super) fn with_photo_measures(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> Self {
        let mut lut = Self::new(r);
        lut.measure(photo_measures(im, r, matrix), r);
        if measures_contrast_pivot(r) {
            lut.photo.contrast_pivot = contrast_pivot(im, r, matrix, &lut.measures);
            lut.basic = basic_tone(r, &lut.photo);
        }
        lut
    }
    /// The scene tone stage for a photo's measures.
    pub(super) fn measure(
        &mut self,
        measures: crate::develop::scene_tone::PhotoMeasures,
        r: &Recipe,
    ) {
        self.measures = measures;
        self.scene = crate::develop::scene_tone::SceneTone::new(r, &measures);
    }
    pub(super) fn new(r: &Recipe) -> Self {
        let photo = crate::develop::basic_tone::PhotoTone {
            contrast_pivot: crate::develop::basic_tone::TYPICAL_PIVOT,
        };
        Self {
            output: PixelOutput::Display,
            exposure_gain: 2f32.powf(r.exposure + r.camera_exposure),
            basic: basic_tone(r, &photo),
            local: None,
            photo,
            mixer: crate::develop::color_mixer::ColorMixer::new(r),
            gray_grid: r
                .effects
                .monochrome
                .then(|| crate::develop::black_white::gray_grid(r.effects.gray_mix)),
            // Camera Raw leaves Point Color out of black & white renders.
            point_colors: (!r.effects.monochrome)
                .then(|| crate::develop::point_color::PointColors::new(&r.point_colors))
                .flatten(),
            grade: crate::develop::color_grade::ColorGrade::new(r),
            rgb_table: r
                .profile
                .as_ref()
                .and_then(|p| p.enhanced.as_ref()?.rgb().cloned()),
            color_adjustments: r.vibrance != 0.
                || r.saturation != 0.
                || r.hsl != [[0.; 3]; 8]
                || r.effects.defringe != [0.; 2]
                || r.effects.monochrome,
            calibration: crate::develop::calibration::Calibration::new(
                r.effects.calibration,
                r.effects.shadow_tint,
            ),
            parametric: parametric_curve(r),
            master: CurveLut::new(&r.curve),
            channels: std::array::from_fn(|c| CurveLut::new(&r.effects.channels[c])),
            measures: Default::default(),
            scene: crate::develop::scene_tone::SceneTone::new(r, &Default::default()),
        }
    }
}
/// The curve after the profile: Contrast. Whites, Blacks and Dehaze are the scene tone
/// stage's.
fn basic_tone(
    r: &Recipe,
    photo: &crate::develop::basic_tone::PhotoTone,
) -> Option<crate::develop::basic_tone::BasicTone> {
    crate::develop::basic_tone::BasicTone::new(r.contrast, photo)
}
/// What the scene tone stage measures of the photo: its reduced copy at the stage's
/// input, with the user's Exposure at 0.
pub(super) fn photo_measures(
    im: Source,
    r: &Recipe,
    matrix: [[f32; 3]; 3],
) -> crate::develop::scene_tone::PhotoMeasures {
    let small = measured_copy(im);
    let base = Recipe {
        exposure: 0.,
        ..r.clone()
    };
    let lut = CurveSet::new(&base);
    // The white point and the photo's maximum are the camera's: measured at the as-shot
    // white balance, so a white balance change keeps them (the synthetic chart's).
    let as_shot = Recipe {
        wb: [1.; 3],
        ..base.clone()
    };
    let scene: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .map(|p| exposure_stage(scene_color(*p, &base, &lut, matrix, None), &lut, None))
        .collect();
    let camera: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .map(|p| exposure_stage(scene_color(*p, &as_shot, &lut, matrix, None), &lut, None))
        .collect();
    let edge = crate::develop::scene_tone::MAX_EDGE;
    let max = box_reduce(&camera, small.width, small.height, edge)
        .into_iter()
        .flatten()
        .fold(f32::MIN_POSITIVE, f32::max);
    let mut lum: Vec<f32> = scene
        .iter()
        .map(|p| crate::develop::scene_tone::luminance(*p))
        .collect();
    let min = lum.iter().copied().fold(f32::INFINITY, f32::min);
    let k = (lum.len().max(1) - 1) * 99 / 100;
    let p99 = if lum.is_empty() {
        1.
    } else {
        *lum.select_nth_unstable_by(k, f32::total_cmp).1
    };
    // A neutral whose last channel just clips (highlight recovery rebuilds the others
    // up to it): the sensor clips at 1 in camera values divided by the as-shot white
    // balance, before the recipe's. Measured on the synthetic chart, whose white
    // balance is not neutral.
    let clip = (0..3)
        .map(|c| im.metadata.wb[c].max(1e-3))
        .fold(0f32, f32::max);
    let white = crate::develop::scene_tone::luminance(exposure_stage(
        scene_color([clip; 3], &as_shot, &lut, matrix, None),
        &lut,
        None,
    ));
    crate::develop::scene_tone::PhotoMeasures {
        sensor_white: white.max(1e-6).log2(),
        // Highlights rebuilt above the sensor's clip do not raise it.
        max: max.min(white).log2(),
        min: min.max(2f32.powi(-20)).log2(),
        p99: crate::develop::local_tone::level(p99),
    }
}
/// `pixels` (`width` × `height`) averaged into boxes so the long edge is `edge`.
fn box_reduce(pixels: &[[f32; 3]], width: u32, height: u32, edge: u32) -> Vec<[f32; 3]> {
    let scale = (edge as f32 / width.max(height) as f32).min(1.);
    let (w, h) = (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    );
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, x1) = (x * width / w, ((x + 1) * width / w).max(x * width / w + 1));
            let (y0, y1) = (
                y * height / h,
                ((y + 1) * height / h).max(y * height / h + 1),
            );
            let mut sum = [0f32; 3];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = pixels[(yy * width + xx) as usize];
                    (0..3).for_each(|c| sum[c] += p[c]);
                }
            }
            sum.map(|v| v / ((x1 - x0) * (y1 - y0)) as f32)
        })
        .collect()
}
/// The measured parametric curve: the user's regions, then (layered) a look's own
/// curve at its Profile Amount, as Camera Raw applies it.
fn parametric_curve(r: &Recipe) -> Option<crate::develop::parametric::ParametricCurve> {
    use crate::develop::parametric::ParametricCurve;
    let user = ParametricCurve::new(r.effects.parametric, r.effects.splits);
    let look = r
        .profile
        .as_ref()
        .and_then(|p| p.enhanced.as_ref())
        .and_then(|look| ParametricCurve::new(look.settings.parametric, look.settings.splits));
    ParametricCurve::then(user, look)
}
/// Whether the recipe's Contrast pivots where the photo's own measure puts it.
pub(crate) fn measures_contrast_pivot(r: &Recipe) -> bool {
    r.contrast != 0.
        || r.masks
            .iter()
            .any(|m| m.is_active() && m.adjust.contrast != 0.)
}
/// The photo reduced for measuring it, without Clarity's and Texture's gain: a user
/// adjustment that depends on the preview size.
fn measured_copy(im: Source<'_>) -> std::borrow::Cow<'_, CameraImage> {
    if let Some(copy) = im.measured {
        return std::borrow::Cow::Borrowed(copy);
    }
    match (im.reduced, im.gain, im.untextured) {
        (Some(small), None, None) => std::borrow::Cow::Borrowed(small),
        _ => std::borrow::Cow::Owned(preview_source(
            Source::new(im.untextured.unwrap_or(im.image), None),
            crate::develop::local_tone::MAP_EDGE,
        )),
    }
}
/// Camera Raw's Contrast pivot for this photo, from its reduced copy rendered as the
/// recipe's profile, white balance and calibration render it, at the camera's
/// exposure: the user's Exposure does not move it (measured on the chart).
fn contrast_pivot(
    im: Source,
    r: &Recipe,
    matrix: [[f32; 3]; 3],
    measures: &crate::develop::scene_tone::PhotoMeasures,
) -> f32 {
    let small = measured_copy(im);
    let default = Recipe {
        exposure: 0.,
        whites: 0.,
        blacks: 0.,
        ..r.clone()
    };
    let mut lut = CurveSet::new(&default);
    lut.measure(*measures, &default);
    let encoded: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .map(|p| {
            tone_stage(*p, &default, &lut, matrix, [0.; 2], None)
                .map(|v| srgb_encode(v.clamp(0., 1.)))
        })
        .collect();
    crate::develop::basic_tone::photo_pivot(&crate::develop::basic_tone::blocks(
        &encoded,
        small.width as usize,
        small.height as usize,
    ))
}
pub(super) fn apply_reference_curves(
    rgb: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> [f32; 3] {
    let p = curve_input(rgb, r, lut, local);
    // No region set: the identity.
    let p = lut.parametric.as_ref().map_or(p, |curve| curve.apply(p));
    let lo = p.into_iter().fold(f32::INFINITY, f32::min);
    let hi = p.into_iter().fold(0f32, f32::max);
    let a = lut.master.evaluate(lo);
    let b = lut.master.evaluate(hi);
    let master = if hi - lo > 1e-8 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [a; 3]
    };
    let master = refine_saturation(p, master, r.curve_saturation);
    let channels = std::array::from_fn(|c| srgb_decode(lut.channels[c].evaluate(master[c])));
    mul(crate::camera_profiles::PRO_TO_RGB, channels)
}

/// The parametric curve's input in each channel of encoded ProPhoto RGB: the basic
/// tone curves, a mask's tone and Levels applied.
pub(super) fn curve_input(
    rgb: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> [f32; 3] {
    let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| srgb_encode(v.clamp(0., 1.)));
    let p = lut.basic.as_ref().map_or(p, |b| b.apply(p));
    let p = match local {
        Some(d) if local::uses(d, &local::TONE_SLOTS) => local::tone(d, p, &lut.photo),
        _ => p,
    };
    p.map(|v| {
        let x = ((v - r.black_point) / (r.white_point - r.black_point))
            .clamp(0., 1.)
            .powf(1. / r.midtone);
        // Contrast renders in the basic tone curves; this is its S-curve at 0, which
        // the GPU port also applies.
        x / (x + (1. - x)).max(1e-8)
    })
}
