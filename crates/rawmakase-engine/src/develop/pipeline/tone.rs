//! Tone preparation: the curve tables a render looks up, and what it measures from the photo first (the contrast pivot, highlights).
use super::*;
use crate::develop::effects::EffectsRendering;

pub(super) struct CurveSet {
    /// Where the stage stops.
    pub(super) output: PixelOutput,
    pub(super) exposure_gain: f32,
    /// Engine 4: Contrast, Whites and Blacks as measured Lightroom curves.
    pub(super) basic_curves: bool,
    pub(super) basic: Option<crate::develop::basic_tone::BasicTone>,
    /// Engine 4 Shadows/Highlights base level, built per image by `with_local`.
    pub(super) local: Option<crate::develop::local_tone::LocalToneMap>,
    /// Engine 4 measured color mixer, Saturation and Vibrance.
    pub(super) mixer: Option<crate::develop::color_mixer::ColorMixer>,
    /// `BlackWhiteModel::Chart`'s grid for the mix (`black_white::gray_grid`).
    pub(crate) gray_grid: Option<Vec<[f32; 3]>>,
    /// Engine 4 Point Color swatches.
    pub(super) point_colors: Option<crate::develop::point_color::PointColors>,
    /// Engine 4 measured color grading, when its settings are covered by the tables.
    pub(super) grade: Option<crate::develop::color_grade::ColorGrade>,
    /// Engine 4: the DNG exposure ramp's black point (Adobe's default Shadows of 5).
    pub(super) black_ramp: Option<ExposureRamp>,
    pub(super) color_adjustments: bool,
    /// The profile look's RGB table, at the recipe's Profile Amount.
    pub(super) rgb_table: Option<crate::camera_profiles::RgbLook>,
    pub(super) calibration: crate::develop::calibration::Calibration,
    /// What Contrast and Whites follow of the photo, for the global sliders and the
    /// masks' (engine 4).
    pub(super) photo: crate::develop::basic_tone::PhotoTone,
    /// Engine 4's measured parametric curve, when the recipe uses it and a region is set.
    pub(super) parametric: Option<crate::develop::parametric::ParametricCurve>,
    pub(super) master: CurveLut,
    pub(super) channels: [CurveLut; 3],
}
impl CurveSet {
    /// Curves plus, for engine 4, the Shadows/Highlights map of this image (which also
    /// carries the measured Clarity); built also when `local_tone` (masks change
    /// Shadows or Highlights).
    pub(super) fn for_image(
        im: Source,
        r: &Recipe,
        matrix: [[f32; 3]; 3],
        local_tone: bool,
    ) -> Self {
        let mut lut = Self::with_photo_measures(im, r, matrix);
        if lut.basic_curves {
            let local = crate::develop::local_tone::LocalToneMap::build(
                im,
                crate::develop::local_tone::Sliders::of(r),
                local_tone,
                |p| tone_stage(p, &im.metadata, r, &lut, matrix, None).0,
            );
            lut.local = local;
        }
        lut
    }
    /// Curves with Contrast at this image's pivot and Whites for its highlights, when
    /// the recipe measures them.
    pub(super) fn with_photo_measures(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> Self {
        Self::with_measures(
            r,
            PhotoMeasures {
                pivot: measures_contrast_pivot(r).then(|| contrast_pivot(im, r, matrix)),
                highlights: measures_whites(r).then(|| photo_highlights(im, r, matrix)),
            },
        )
    }
    /// Curves with the photo's measures already taken (see [`PhotoMeasures`]).
    pub(crate) fn with_measures(r: &Recipe, measures: PhotoMeasures) -> Self {
        let mut photo = Self::photo_tone(r);
        if let Some(pivot) = measures.pivot {
            photo.contrast = crate::develop::basic_tone::ContrastCurve::Pivot(pivot);
        }
        if let Some(highlights) = measures.highlights {
            photo.whites = crate::develop::basic_tone::WhitesTable::for_highlights(highlights);
        }
        let measured = measures.pivot.is_some() || measures.highlights.is_some();
        Self::with_photo(r, photo, measured)
    }
    pub(super) fn new(r: &Recipe) -> Self {
        Self::with_photo(r, Self::photo_tone(r), false)
    }
    /// What the Basic curve takes from the photo before it is measured.
    fn photo_tone(r: &Recipe) -> crate::develop::basic_tone::PhotoTone {
        crate::develop::basic_tone::PhotoTone {
            contrast: match r.contrast_model {
                crate::model::operators::ContrastModel::Original => {
                    crate::develop::basic_tone::ContrastCurve::Original
                }
                crate::model::operators::ContrastModel::Adaptive => {
                    crate::develop::basic_tone::ContrastCurve::Pivot(
                        crate::develop::basic_tone::TYPICAL_PIVOT,
                    )
                }
            },
            // Without the photo, adaptive Whites takes the original median curve.
            whites: crate::develop::basic_tone::WhitesTable::original(),
        }
    }
    /// The curves with the Basic curve built for `photo`; `measured` builds it even
    /// where the recipe does not use the reference curves.
    fn with_photo(
        r: &Recipe,
        photo: crate::develop::basic_tone::PhotoTone,
        measured: bool,
    ) -> Self {
        let basic_curves = r.engine >= 4 && r.reference_curves;
        Self {
            output: PixelOutput::Display,
            exposure_gain: 2f32.powf(r.exposure + r.camera_exposure),
            basic_curves,
            basic: (basic_curves || measured)
                .then(|| {
                    crate::develop::basic_tone::BasicTone::new(
                        r.contrast,
                        r.whites,
                        r.blacks,
                        r.effects.dehaze,
                        &photo,
                    )
                })
                .flatten(),
            local: None,
            photo,
            mixer: basic_curves
                .then(|| crate::develop::color_mixer::ColorMixer::new(r))
                .flatten(),
            gray_grid: (r.effects.monochrome
                && r.black_white_model == crate::model::operators::BlackWhiteModel::Chart)
                .then(|| crate::develop::black_white::gray_grid(r.effects.gray_mix)),
            // Camera Raw leaves Point Color out of black & white renders.
            point_colors: (basic_curves && !r.effects.monochrome)
                .then(|| crate::develop::point_color::PointColors::new(&r.point_colors))
                .flatten(),
            grade: (basic_curves && r.reference_color)
                .then(|| crate::develop::color_grade::ColorGrade::new(r))
                .flatten(),
            black_ramp: basic_curves.then(|| {
                ExposureRamp::new(default_black(r) * 2f32.powf(r.exposure + r.camera_exposure))
            }),
            rgb_table: r
                .profile
                .as_ref()
                .filter(|_| r.engine >= 3)
                .and_then(|p| p.enhanced.as_ref()?.rgb().cloned()),
            color_adjustments: r.vibrance != 0.
                || r.saturation != 0.
                || r.hsl != [[0.; 3]; 8]
                || r.effects.defringe != [0.; 2]
                || r.effects.monochrome,
            calibration: crate::develop::calibration::Calibration::new(
                r.effects.calibration,
                r.effects.shadow_tint,
                r.calibration_model,
            ),
            parametric: (basic_curves && r.parametric_model.is_measured())
                .then(|| parametric_curve(r))
                .flatten(),
            master: CurveLut::new(&r.curve),
            channels: std::array::from_fn(|c| CurveLut::new(&r.effects.channels[c])),
        }
    }
}
/// The measured parametric curve: the user's regions, then (layered) a look's own
/// curve at its Profile Amount, as Camera Raw applies it.
fn parametric_curve(r: &Recipe) -> Option<crate::develop::parametric::ParametricCurve> {
    use crate::develop::parametric::ParametricCurve;
    use crate::model::operators::ParametricModel;
    let user = ParametricCurve::new(r.effects.parametric, r.effects.splits);
    let look = r
        .profile
        .as_ref()
        .filter(|_| r.parametric_model == ParametricModel::Layered)
        .and_then(|p| p.enhanced.as_ref())
        .and_then(|look| ParametricCurve::new(look.settings.parametric, look.settings.splits));
    ParametricCurve::then(user, look)
}
/// What a render measures of the photo before building its curves: the Contrast pivot
/// and the highlights positive Whites follows, each when the recipe uses it. Each depends
/// only on the photo and the tone stage (see `stage_cache::ToneKey`), so previews keep
/// them while the Basic sliders move.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PhotoMeasures {
    pub(crate) pivot: Option<f32>,
    pub(crate) highlights: Option<f32>,
}
/// [`contrast_pivot`] with the recipe's own profile matrix.
pub(crate) fn measured_pivot(im: Source, r: &Recipe) -> f32 {
    contrast_pivot(im, r, profile_matrix(&im.metadata, r))
}
/// [`photo_highlights`] with the recipe's own profile matrix.
pub(crate) fn measured_highlights(im: Source, r: &Recipe) -> f32 {
    photo_highlights(im, r, profile_matrix(&im.metadata, r))
}
/// Whether the recipe's Contrast pivots where the photo's own measure puts it.
pub(crate) fn measures_contrast_pivot(r: &Recipe) -> bool {
    r.engine >= 4
        && r.reference_curves
        && r.contrast_model == crate::model::operators::ContrastModel::Adaptive
        && (r.contrast != 0.
            || r.masks
                .iter()
                .any(|m| m.is_active() && m.adjust.contrast != 0.))
}
/// Whether the recipe's positive Whites follows the photo's highlights.
pub(crate) fn measures_whites(r: &Recipe) -> bool {
    r.engine >= 4
        && r.reference_curves
        && r.whites_model == crate::model::operators::WhitesModel::Adaptive
        && (r.whites > 0.
            || r.masks
                .iter()
                .any(|m| m.is_active() && m.adjust.whites > 0.))
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
            let rgb = tone_stage(*p, &im.metadata, r, &lut, matrix, None).0;
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
        .map(|p| {
            tone_stage(*p, &im.metadata, &default, &lut, matrix, None)
                .0
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
    let p = match &lut.parametric {
        Some(curve) => curve.apply(p),
        // The original curve, the identity when no region is set.
        None => p.map(|x| r.effects.parametric(x)),
    };
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
/// tone curves, a mask's tone, Levels and (before engine 4) Contrast applied.
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
    let contrast = if lut.basic_curves { 0. } else { r.contrast };
    p.map(|v| {
        let x = ((v - r.black_point) / (r.white_point - r.black_point))
            .clamp(0., 1.)
            .powf(1. / r.midtone);
        let power = 2f32.powf(contrast);
        let low = x.powf(power);
        low / (low + (1. - x).powf(power)).max(1e-8)
    })
}

pub(super) fn apply_curve(encoded: f32, c: usize, r: &Recipe, lut: &CurveSet) -> f32 {
    let level = ((encoded - r.black_point) / (r.white_point - r.black_point))
        .clamp(0., 1.)
        .powf(1. / r.midtone);
    let contrast = if r.wide_gamut_curves && r.contrast != 0. {
        // A bounded S-curve preserves black/white endpoints and avoids the
        // premature clipping of the legacy affine contrast adjustment.
        let power = 2f32.powf(r.contrast);
        let low = level.powf(power);
        low / (low + (1. - level).powf(power)).max(1e-8)
    } else {
        ((level - 0.5) * (1. + r.contrast) + 0.5).clamp(0., 1.)
    };
    let master = lut.master.evaluate(r.effects.parametric(contrast));
    let curve = &r.effects.channels[c];
    if curve.points == [[0., 0.], [1., 1.]] {
        master
    } else {
        lut.channels[c].evaluate(master)
    }
}
