//! Tone preparation: the curve tables a render looks up, and what it measures from the photo first (the contrast pivot, highlights).
use super::*;

pub(super) struct CurveSet {
    /// Where the stage stops.
    pub(super) output: PixelOutput,
    pub(super) exposure_gain: f32,
    /// Dehaze, Contrast, Whites and Blacks as measured Lightroom curves.
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
    /// The DNG exposure ramp's black point (Adobe's default Shadows of 5).
    pub(super) black_ramp: Option<ExposureRamp>,
    pub(super) color_adjustments: bool,
    /// The profile look's RGB table, at the recipe's Profile Amount.
    pub(super) rgb_table: Option<crate::camera_profiles::RgbLook>,
    pub(super) calibration: crate::develop::calibration::Calibration,
    /// What Contrast and Whites follow of the photo, for the global sliders and the
    /// masks'.
    pub(super) photo: crate::develop::basic_tone::PhotoTone,
    /// The measured parametric curve, when a region is set.
    pub(super) parametric: Option<crate::develop::parametric::ParametricCurve>,
    pub(super) master: CurveLut,
    pub(super) channels: [CurveLut; 3],
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
        lut.local = crate::develop::local_tone::LocalToneMap::build(
            im,
            crate::develop::local_tone::Sliders::of(r),
            local_tone,
            |p| tone_stage(p, r, &lut, matrix, None),
        );
        lut
    }
    /// Curves with Contrast at this image's pivot and Whites for its highlights, when
    /// the recipe measures them.
    pub(super) fn with_photo_measures(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> Self {
        let mut lut = Self::new(r);
        let (pivot, whites) = (measures_contrast_pivot(r), measures_whites(r));
        if pivot {
            lut.photo.contrast_pivot = contrast_pivot(im, r, matrix);
        }
        if whites {
            lut.photo.whites = crate::develop::basic_tone::WhitesTable::for_highlights(
                photo_highlights(im, r, matrix),
            );
        }
        if pivot || whites {
            lut.basic = crate::develop::basic_tone::BasicTone::new(
                r.contrast,
                r.whites,
                r.blacks,
                r.effects.dehaze,
                &lut.photo,
            );
        }
        lut
    }
    pub(super) fn new(r: &Recipe) -> Self {
        let photo = crate::develop::basic_tone::PhotoTone {
            contrast_pivot: crate::develop::basic_tone::TYPICAL_PIVOT,
            // Without the photo, Whites takes the median curve.
            whites: crate::develop::basic_tone::WhitesTable::original(),
        };
        Self {
            output: PixelOutput::Display,
            exposure_gain: 2f32.powf(r.exposure + r.camera_exposure),
            basic: crate::develop::basic_tone::BasicTone::new(
                r.contrast,
                r.whites,
                r.blacks,
                r.effects.dehaze,
                &photo,
            ),
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
            black_ramp: Some(ExposureRamp::new(
                default_black(r) * 2f32.powf(r.exposure + r.camera_exposure),
            )),
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
        }
    }
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
/// Whether the recipe's positive Whites follows the photo's highlights.
pub(crate) fn measures_whites(r: &Recipe) -> bool {
    r.whites > 0.
        || r.masks
            .iter()
            .any(|m| m.is_active() && m.adjust.whites > 0.)
}
/// The photo reduced for measuring it, without Clarity's and Texture's gain: a user
/// adjustment that depends on the preview size.
fn measured_copy(im: Source<'_>) -> std::borrow::Cow<'_, CameraImage> {
    match (im.reduced, im.gain, im.untextured) {
        (Some(small), None, None) => std::borrow::Cow::Borrowed(small),
        _ => std::borrow::Cow::Owned(preview_source(
            Source::new(im.untextured.unwrap_or(im.image), None),
            crate::develop::local_tone::MAP_EDGE,
        )),
    }
}
/// The highlights positive Whites follows: the 98th percentile of the encoded
/// luminance of the photo's reduced copy as the recipe renders it before the Basic
/// tone sliders, its Exposure included (measured on the chart).
fn photo_highlights(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> f32 {
    let small = measured_copy(im);
    let lut = CurveSet::new(r);
    let luminance: Vec<f32> = small
        .pixels
        .par_iter()
        .map(|p| {
            let rgb = tone_stage(*p, r, &lut, matrix, None);
            srgb_encode(crate::develop::local_tone::luminance(rgb).clamp(0., 1.))
        })
        .collect();
    crate::develop::basic_tone::highlights(luminance)
}
/// Camera Raw's Contrast pivot for this photo, from its reduced copy rendered as the
/// recipe's profile, white balance and calibration render it, at the camera's
/// exposure: the user's Exposure does not move it (measured on the chart).
fn contrast_pivot(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> f32 {
    let small = measured_copy(im);
    let default = Recipe {
        exposure: 0.,
        ..r.clone()
    };
    let lut = CurveSet::new(&default);
    let encoded: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .map(|p| tone_stage(*p, &default, &lut, matrix, None).map(|v| srgb_encode(v.clamp(0., 1.))))
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
