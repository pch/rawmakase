//! Tone preparation: the curve tables a render looks up, and what it measures from the photo first (the scene tone stage's measures, the contrast pivot).
use super::*;

pub(super) struct CurveSet {
    /// Where the stage stops.
    pub(super) output: PixelOutput,
    /// The colour space the finished pixels are encoded in: sRGB but for an export
    /// in another space.
    pub(super) space: crate::color::OutputSpace,
    pub(super) exposure_gain: f32,
    /// Contrast as a measured Camera Raw curve.
    pub(super) basic: Option<crate::develop::tone_lut::ToneLut>,
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
    pub(super) parametric: Option<crate::develop::tone_lut::ToneLut>,
    pub(super) master: CurveLut,
    pub(super) channels: [CurveLut; 3],
    /// What the scene tone stage measured of the photo.
    pub(crate) measures: crate::develop::scene_tone::PhotoMeasures,
    /// The scene tone stage (S3) for these measures.
    pub(super) scene: crate::develop::scene_tone::SceneTone,
    /// An HDR render's profile stage and limit; `None` renders SDR.
    pub(super) hdr: Option<HdrFinish>,
    /// The SDR rendition's sliders, on an HDR edit's SDR rendition.
    pub(super) sdr_rendition: Option<crate::develop::sdr_rendition::SdrRenditionTone>,
}
/// How an HDR render finishes after the scene tone stage: the profile's tables and
/// curve, and its look's, unclipped ([`crate::camera_profiles::CameraProfile::finish_hdr`]),
/// then Contrast ([`crate::develop::basic_tone::HdrContrast`]), the tone curves
/// ([`HdrCurves`]), the colour mixer, Saturation and Vibrance ([`CurveSet::mixer`]),
/// colour grading (`ColorGrade::apply_hdr`) and the black & white mix
/// ([`CurveSet::gray_grid`]) as SDR renders apply them, above white too
/// (docs/hdr.md#the-colour-stage). The colour stage's other controls are not
/// rendered: an HDR edit that uses them does not validate (`model::hdr::check_recipe`).
pub(super) struct HdrFinish {
    tone: crate::camera_profiles::hdr_tone::HdrTone,
    look: Option<crate::camera_profiles::hdr_look::LookTone>,
    /// Contrast at the photo's pivot, set once the pivot is measured.
    contrast: Option<crate::develop::basic_tone::HdrContrast>,
    /// `None` when every curve is the identity, so the master is the profile's.
    curves: Option<HdrCurves>,
}
/// The user's tone curves as they render in HDR (`color::hdr_curve`): Levels, the
/// parametric curve, the master point curve, then each channel's point curve, as in
/// SDR.
struct HdrCurves {
    /// Black point, white point and midtone, when they are not the default.
    levels: Option<[f32; 3]>,
    parametric: Option<crate::develop::tone_lut::ToneLut>,
    master: crate::color::hdr_curve::HdrPointCurve,
    /// Red, green and blue's own curves, each when it is not the identity; when none
    /// is, `None`.
    channels: Option<[Option<crate::color::hdr_curve::HdrPointCurve>; 3]>,
}
impl HdrCurves {
    fn new(r: &Recipe) -> Option<Self> {
        use crate::color::hdr_curve::HdrPointCurve;
        let parametric = parametric_curve(r);
        let levels = [r.black_point, r.white_point, r.midtone];
        let levels = (levels != [0., 1., 1.]).then_some(levels);
        let channels = (!r.effects.channels.iter().all(|c| c.is_identity())).then(|| {
            r.effects
                .channels
                .each_ref()
                .map(|c| (!c.is_identity()).then(|| HdrPointCurve::new(c)))
        });
        if levels.is_none() && parametric.is_none() && r.curve.is_identity() && channels.is_none() {
            return None;
        }
        Some(Self {
            levels,
            parametric,
            master: HdrPointCurve::new(&r.curve),
            channels,
        })
    }
    /// Linear ProPhoto RGB (1 = graphics white, not negative) through the curves.
    fn apply(&self, pro: [f32; 3]) -> [f32; 3] {
        use crate::color::hdr_curve::{CEILING, channel, decode, encode, levels, parametric};
        let p = pro.map(encode);
        let p = match self.levels {
            Some([black, white, midtone]) => p.map(|e| levels(e, black, white, midtone)),
            None => p,
        };
        let p = match &self.parametric {
            Some(curve) => rgb_tone(p, |e| parametric(e, |x| curve.eval(x))),
            None => p,
        };
        let p = rgb_tone(p, |e| self.master.evaluate(e));
        // An identity channel curve leaves its channel as the master curve renders it.
        let channels = self.channels.as_ref();
        std::array::from_fn(|c| match channels.and_then(|curves| curves[c].as_ref()) {
            Some(curve) => channel(curve, p[c]),
            None => decode(p[c]).min(CEILING),
        })
    }
}
/// Point Color on an HDR master's linear display RGB (1 = graphics white). Camera Raw
/// 18.7 applies its swatches to each channel taken into colour grading's HDR domain
/// (`color_grade_curves::to_hdr_domain`, 16 times white at 1) and back, where they
/// clip at 1 as SDR's clip at white (docs/hdr.md#point-color). A pixel brighter than 16
/// is taken with its brightest channel at 16 and scaled back.
fn point_colors_hdr(p: &crate::develop::point_color::PointColors, rgb: [f32; 3]) -> [f32; 3] {
    use crate::develop::color_grade_curves::{HDR_TOP, from_hdr_domain, to_hdr_domain};
    let pro = mul(crate::camera_profiles::RGB_TO_PRO, rgb);
    let k = pro.into_iter().fold(HDR_TOP, f32::max) / HDR_TOP;
    let u = pro.map(|v| if v > 0. { to_hdr_domain(v / k) } else { v / k });
    let out = p.render_prophoto(u).color;
    let pro = out.map(|u| {
        if u > 0. {
            from_hdr_domain(u.min(1.)) * k
        } else {
            u * k
        }
    });
    mul(crate::camera_profiles::PRO_TO_RGB, pro)
}
/// What the HDR master of `r` renders a neutral of scene value `x` as (linear
/// ProPhoto entering the scene tone stage, after Exposure), as its luminance: the scene
/// tone stage's global curves, its toe and its Whites and Blacks, then the profile, its
/// look, Contrast, the tone curves and the colour stage, without local adjustments (the
/// curves are the render's own, [`CurveSet::for_image`], so Shadows' black shift is
/// in them). That is the HDR response Highlight Priority's vignette exposes through
/// (docs/hdr.md#vignette-and-grain). `None` when `r` renders no master.
pub(crate) fn hdr_neutral_response(im: Source, r: &Recipe) -> Option<impl Fn(f32) -> f32> {
    let camera = camera_stage(&im.image.metadata, r);
    let lut = CurveSet::for_image(im, r, camera, false);
    lut.hdr.as_ref()?;
    Some(move |x: f32| {
        let scene = lut.scene.apply([x.max(0.); 3], [0.5, 0.5], 1., None);
        let hdr = lut.hdr.as_ref().expect("checked above");
        crate::develop::scene_tone::luminance(hdr.finish(scene, r, &lut))
    })
}
/// DNG RGBTone with `curve`: it maps the largest and smallest channel, and the middle
/// one keeps its relative position, as the SDR curves apply.
fn rgb_tone(p: [f32; 3], curve: impl Fn(f32) -> f32) -> [f32; 3] {
    let lo = p.into_iter().fold(f32::INFINITY, f32::min);
    let hi = p.into_iter().fold(0f32, f32::max);
    let (a, b) = (curve(lo), curve(hi));
    if hi - lo > 1e-8 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [b; 3]
    }
}
/// What the GPU port's HDR master reads of [`HdrFinish`] past the profile: Contrast's
/// table, output at its top and slope beyond; and, with tone curves, whether Levels
/// apply and the master curve's value at white and slope beyond it, and each channel
/// curve's that is not the identity, when any is not.
pub(crate) struct HdrFinishGpu<'a> {
    pub(crate) contrast: Option<(&'a [f32], f32, f32)>,
    pub(crate) curves: Option<(bool, [f32; 2])>,
    pub(crate) channels: Option<[Option<[f32; 2]>; 3]>,
}
impl HdrFinish {
    pub(super) fn gpu(&self) -> HdrFinishGpu<'_> {
        HdrFinishGpu {
            contrast: self.contrast.as_ref().map(|c| c.gpu()),
            curves: self
                .curves
                .as_ref()
                .map(|c| (c.levels.is_some(), c.master.extension())),
            channels: self
                .curves
                .as_ref()
                .and_then(|c| c.channels.as_ref())
                .map(|curves| curves.each_ref().map(|c| c.as_ref().map(|c| c.extension()))),
        }
    }
}
impl HdrFinish {
    /// For a recipe with HDR on and a profile whose curve renders in HDR.
    pub(super) fn new(r: &Recipe) -> Option<Self> {
        if !r.hdr.is_active() {
            return None;
        }
        let profile = r.profile.as_ref()?;
        Some(Self {
            tone: profile.hdr_tone().ok()?,
            look: profile.hdr_look(),
            contrast: None,
            curves: HdrCurves::new(r),
        })
    }
    /// Linear ProPhoto RGB from the scene tone stage to the HDR master before its
    /// limit: linear ProPhoto RGB, 1 = graphics white, not negative.
    pub(super) fn finish(&self, scene: [f32; 3], r: &Recipe, lut: &CurveSet) -> [f32; 3] {
        let rgb = match &r.profile {
            Some(profile) => profile.finish_hdr(scene, &self.tone, self.look.as_ref()),
            None => mul(crate::camera_profiles::PRO_TO_RGB, scene),
        };
        let pro = mul(crate::camera_profiles::RGB_TO_PRO, rgb);
        // Contrast follows the profile and its look, as in SDR's colour stage, and the
        // curves follow Contrast, as SDR's `curve_input` applies them.
        let pro = match &self.contrast {
            Some(contrast) => contrast.apply(pro.map(|v| v.max(0.))),
            None => pro,
        }
        .map(|v| v.max(0.));
        let pro = self.curves.as_ref().map_or(pro, |c| c.apply(pro));
        // The colour mixer and the black & white mix follow Contrast and the tone
        // curves, as in SDR's colour stage.
        if lut.mixer.is_none()
            && lut.point_colors.is_none()
            && lut.rgb_table.is_none()
            && lut.grade.is_none()
            && lut.gray_grid.is_none()
        {
            // The limit applies once, after sharpening and resizing (`render_hdr`).
            return pro;
        }
        let rgb = mul(crate::camera_profiles::PRO_TO_RGB, pro);
        // Band Luminance fades by the scene's level, before the profile's curve.
        let rgb = lut
            .mixer
            .as_ref()
            .map_or(rgb, |m| m.apply_at(rgb, Some(scene)));
        // Point Color follows the mixer, as in SDR.
        let rgb = lut
            .point_colors
            .as_ref()
            .map_or(rgb, |p| point_colors_hdr(p, rgb));
        // A look's RGB table follows the mixer, as in SDR, on overrange-encoded values.
        let rgb = lut.rgb_table.as_ref().map_or(rgb, |t| t.apply_hdr(rgb));
        // Colour grading follows the colour mixer and a look's RGB table, as in SDR.
        let rgb = lut.grade.as_ref().map_or(rgb, |g| g.apply_hdr(rgb));
        // The gray's luminance, unclipped: SDR renders clip it at white.
        let rgb = lut.gray_grid.as_ref().map_or(rgb, |g| {
            let lookup = crate::develop::color_mixer::Lookup::Hdr(None, None);
            [crate::develop::black_white::gray_in(g, rgb, lookup); 3]
        });
        mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| v.max(0.))
    }
    #[cfg(test)]
    pub(super) fn has_contrast(&self) -> bool {
        self.contrast.is_some()
    }
}
impl CurveSet {
    /// Curves plus the Shadows/Highlights map of this image (which also carries the
    /// measured Clarity); built also when `local_tone` (masks change Shadows or
    /// Highlights).
    pub(super) fn for_image(im: Source, r: &Recipe, camera: Camera, local_tone: bool) -> Self {
        let (mut lut, measured) = Self::measured(im, r, camera);
        let small = measured_copy(im);
        let sliders = crate::develop::local_tone::Sliders::of(r);
        // The base reads what the measures read, and Exposure.
        let base =
            crate::develop::local_tone::LocalToneMap::needed(sliders, local_tone).then(|| {
                let scene = |color| {
                    crate::develop::scene_tone::luminance(exposure_stage(color, &lut, None))
                };
                let size = [small.width, small.height];
                match &measured {
                    // The copy's camera colour reads only what the measures read: kept
                    // with them, so an Exposure change only exposes it.
                    Some(m) => m.map_base(r.exposure, || {
                        let colors = m.colors(|| {
                            small
                                .pixels
                                .par_iter()
                                .map(|p| scene_color(*p, r, &lut, camera, None))
                                .collect()
                        });
                        let lum = colors.par_iter().map(|c| scene(*c)).collect();
                        crate::develop::local_tone::MapBase::new(lum, size)
                    }),
                    None => Arc::new(crate::develop::local_tone::MapBase::of(&small, |p| {
                        scene(scene_color(p, r, &lut, camera, None))
                    })),
                }
            });
        lut.local = base.as_ref().map(|base| {
            crate::develop::local_tone::LocalToneMap::from_base(
                base,
                [im.width, im.height],
                sliders,
                local_tone,
            )
        });
        if (r.shadows > 0. || local_tone)
            && let (Some(map), Some(base)) = (&lut.local, &base)
        {
            // The black response follows the median positive-Shadows gain. Masks
            // select this response locally; an unmasked pixel keeps the global key.
            let levels = base.lum();
            let shift = |shadows: f32| {
                if shadows <= 0. {
                    return 0.;
                }
                let mut gains: Vec<f32> = levels
                    .par_iter()
                    .enumerate()
                    .map(|(i, y)| {
                        let x = (i % small.width as usize) as f32 / map.scale[0];
                        let yy = (i / small.width as usize) as f32 / map.scale[1];
                        (map.gain_with(x, yy, *y, *y, [shadows, 0., 0.])
                            / map.gain_with(x, yy, *y, *y, [0., 0., 0.]))
                        .log2()
                    })
                    .collect();
                let k = (gains.len() - 1) / 2;
                gains.select_nth_unstable_by(k, f32::total_cmp);
                gains[k]
            };
            let mut adjusted = lut.measures;
            adjusted.dark += shift(r.shadows);
            let mask_keys = local_tone.then(|| {
                std::array::from_fn(|i| lut.measures.dark_key(r.exposure) + shift(i as f32 / 10.))
            });
            let haze = lut.scene.haze.take();
            lut.scene = crate::develop::scene_tone::SceneTone::new(r, &adjusted, haze);
            if let Some(keys) = mask_keys {
                lut.scene.global.set_shadow_black(r.shadows, &keys);
            }
        }
        lut
    }
    /// Curves with the scene tone stage for this image's measures, and Contrast at
    /// its pivot when the recipe measures it.
    #[cfg(test)]
    pub(super) fn with_photo_measures(im: Source, r: &Recipe, camera: Camera) -> Self {
        Self::measured(im, r, camera).0
    }
    /// Curves with the scene tone stage for this image's measures, and Contrast at
    /// its pivot when the recipe measures it, with the stage cache's entry for the
    /// measures when there is one.
    fn measured(
        im: Source,
        r: &Recipe,
        camera: Camera,
    ) -> (Self, Option<Arc<crate::develop::scene_tone::Measured>>) {
        let mut lut = Self::new(r);
        let (measures, haze, measured) = cached_photo_measures(im, r, camera);
        let dehazing = haze.as_ref().map(|haze| {
            crate::develop::scene_tone::Dehazing::new(
                haze.clone(),
                [im.width, im.height],
                r.exposure,
            )
        });
        lut.measure(measures, dehazing, r);
        if measures_contrast_pivot(r) {
            // It reads what the measures read, and Dehaze.
            let measure = || contrast_pivot(im, r, camera, &lut.measures, haze);
            lut.photo.contrast_pivot = match &measured {
                Some(m) => m.pivot(r.effects.dehaze, measure),
                None => measure(),
            };
            lut.basic = basic_tone(r, &lut.photo);
            if let Some(hdr) = &mut lut.hdr {
                hdr.contrast = crate::develop::basic_tone::HdrContrast::new(r.contrast, &lut.photo);
            }
        }
        (lut, measured)
    }
    /// The scene tone stage for a photo's measures, and its haze as this render removes
    /// it.
    pub(super) fn measure(
        &mut self,
        measures: crate::develop::scene_tone::PhotoMeasures,
        haze: Option<crate::develop::scene_tone::Dehazing>,
        r: &Recipe,
    ) {
        self.measures = measures;
        self.scene = crate::develop::scene_tone::SceneTone::new(r, &measures, haze);
    }
    pub(super) fn new(r: &Recipe) -> Self {
        let photo = crate::develop::basic_tone::PhotoTone {
            contrast_pivot: crate::develop::basic_tone::TYPICAL_PIVOT,
        };
        Self {
            output: PixelOutput::Display,
            space: crate::color::OutputSpace::Srgb,
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
            scene: crate::develop::scene_tone::SceneTone::new(r, &Default::default(), None),
            hdr: HdrFinish::new(r),
            // Only an HDR edit's SDR rendition keeps them (`Recipe::resolved`).
            sdr_rendition: (!r.hdr.enabled)
                .then(|| crate::develop::sdr_rendition::SdrRenditionTone::new(&r.hdr.sdr))
                .flatten(),
        }
    }
}
/// The curve after the profile: Contrast. Whites, Blacks and Dehaze are the scene tone
/// stage's.
fn basic_tone(
    r: &Recipe,
    photo: &crate::develop::basic_tone::PhotoTone,
) -> Option<crate::develop::tone_lut::ToneLut> {
    crate::develop::basic_tone::contrast_curve(r.contrast, photo)
}
/// `photo_measures`, and `photo_haze` when the recipe removes haze
/// (`scene_tone::needs_haze`), through the stage cache when the source has its
/// measurement copy and the cache: kept per copy, matrix and the recipe fields they
/// read, so Exposure, tone and Dehaze edits reuse them. With the cache, also its entry,
/// which keeps what else is measured on the same scene values.
fn cached_photo_measures(
    im: Source,
    r: &Recipe,
    camera: Camera,
) -> (
    crate::develop::scene_tone::PhotoMeasures,
    Option<Arc<crate::develop::scene_tone::Haze>>,
    Option<Arc<crate::develop::scene_tone::Measured>>,
) {
    let needs_haze = crate::develop::scene_tone::needs_haze(r);
    let (Some(copy), Some(cache)) = (im.measured, im.measures) else {
        return (
            photo_measures(im, r, camera),
            needs_haze.then(|| Arc::new(photo_haze(im, r, camera))),
            None,
        );
    };
    let key = crate::develop::stage_cache::MeasuresKey::new(copy, im.unretouched, r, camera);
    // Not locked while measuring, whose parallel work may run other renders' tasks.
    let cache = || {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let cached = cache().get(&key);
    let measured = match cached {
        Some(measured) => measured,
        None => {
            let measured = Arc::new(crate::develop::scene_tone::Measured::new(photo_measures(
                im, r, camera,
            )));
            cache().insert(
                key,
                measured.clone(),
                std::mem::size_of::<crate::develop::scene_tone::Measured>(),
            );
            measured
        }
    };
    let haze = needs_haze.then(|| {
        if let Some(haze) = measured.haze.get() {
            return haze.clone();
        }
        // Not initialised in place either: a render that waited on it could be the
        // task this measure's parallel work runs. Renders measuring it at the same time
        // compute the same haze; the first one stored is kept.
        let haze = Arc::new(photo_haze(im, r, camera));
        measured.haze.get_or_init(|| haze).clone()
    });
    (measured.measures, haze, Some(measured))
}
/// What the scene tone stage measures of the photo: its reduced copy at the stage's
/// input, with the user's Exposure at 0. Reads only the copy (and its metadata), `matrix`
/// and the recipe fields `stage_cache::MeasuresKey` holds.
pub(super) fn photo_measures(
    im: Source,
    r: &Recipe,
    camera: Camera,
) -> crate::develop::scene_tone::PhotoMeasures {
    let small = measured_copy(im);
    let base = Recipe {
        exposure: 0.,
        ..r.clone()
    };
    let lut = CurveSet::new(&base);
    let scene = measured_scene(&small, &base, &lut, camera);
    // The copy without retouching, when it differs, for the darkest level.
    let mut unretouched: Vec<f32> = match (im.measured, im.unretouched) {
        (Some(copy), Some(plain)) if !Arc::ptr_eq(copy, plain) => {
            measured_scene(plain, &base, &lut, camera)
                .into_iter()
                .map(crate::develop::scene_tone::luminance)
                .collect()
        }
        _ => Vec::new(),
    };
    // The white point and the photo's maximum are the camera's: measured at the as-shot
    // white balance, so a white balance change keeps them (the synthetic chart's). A
    // profile's matrices follow the temperature, so its as-shot one is used too.
    let metadata = &small.metadata;
    let as_shot = Recipe {
        wb: [1.; 3],
        incremental_temperature: 0.,
        incremental_tint: 0.,
        temperature: r
            .profile
            .as_ref()
            .and_then(|p| p.as_shot_white_balance(metadata))
            .map_or(r.temperature, |[temperature, _]| temperature),
        ..base
    };
    let shot_camera = super::camera_stage(metadata, &as_shot);
    let camera: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .map(|p| {
            exposure_stage(
                scene_color(*p, &as_shot, &lut, shot_camera, None),
                &lut,
                None,
            )
        })
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
    // The 0.1th percentile, before red eye corrections and spot removal: a few dark
    // pixels do not move it, and a local repair does not move the whole photo's black.
    let dark_lum = if unretouched.is_empty() {
        &mut lum
    } else {
        &mut unretouched
    };
    let dark = if dark_lum.is_empty() {
        min
    } else {
        let k = (dark_lum.len() - 1) / 1000;
        *dark_lum.select_nth_unstable_by(k, f32::total_cmp).1
    };
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
        .map(|c| metadata.wb[c].max(1e-3))
        .fold(0f32, f32::max);
    let white = crate::develop::scene_tone::luminance(exposure_stage(
        scene_color([clip; 3], &as_shot, &lut, shot_camera, None),
        &lut,
        None,
    ));
    // A rendered file's white curve is Camera Raw's default rendering at keys 0, which
    // its pseudo-scene was made with (`develop::rendered`), whatever the scene measures:
    // so the file renders back as it is at default settings.
    let rendered = metadata.is_rendered();
    crate::develop::scene_tone::PhotoMeasures {
        sensor_white: if rendered { 0. } else { white.max(1e-6).log2() },
        // Highlights rebuilt above the sensor's clip do not raise it.
        max: if rendered { 0. } else { max.min(white).log2() },
        min: min.max(2f32.powi(-20)).log2(),
        dark: dark.max(2f32.powi(-20)).log2(),
        p99: crate::develop::local_tone::level(p99),
    }
}
/// The haze positive Dehaze removes (`scene_tone::Haze`), on the scene values
/// `photo_measures` reads: the measurement copy at the stage's input, Exposure 0. Reads
/// what `photo_measures` reads, so it is kept with them.
pub(super) fn photo_haze(
    im: Source,
    r: &Recipe,
    camera: Camera,
) -> crate::develop::scene_tone::Haze {
    let small = measured_copy(im);
    let base = Recipe {
        exposure: 0.,
        ..r.clone()
    };
    let scene = measured_scene(&small, &base, &CurveSet::new(&base), camera);
    crate::develop::scene_tone::Haze::of(&scene, small.width as usize, small.height as usize)
}
/// The measurement copy `small` at the scene tone stage's input for `base` (the recipe
/// with Exposure 0) and its curves.
fn measured_scene(
    small: &CameraImage,
    base: &Recipe,
    lut: &CurveSet,
    camera: Camera,
) -> Vec<[f32; 3]> {
    small
        .pixels
        .par_iter()
        .map(|p| exposure_stage(scene_color(*p, base, lut, camera, None), lut, None))
        .collect()
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
fn parametric_curve(r: &Recipe) -> Option<crate::develop::tone_lut::ToneLut> {
    use crate::develop::{parametric::curve, tone_lut::ToneLut};
    let user = curve(r.effects.parametric, r.effects.splits);
    let look = r
        .profile
        .as_ref()
        .and_then(|p| p.enhanced.as_ref())
        .and_then(|look| curve(look.settings.parametric, look.settings.splits));
    ToneLut::then(user, look)
}
/// Whether the recipe's Contrast pivots where the photo's own measure puts it.
pub(crate) fn measures_contrast_pivot(r: &Recipe) -> bool {
    r.contrast != 0.
        || r.masks
            .iter()
            .any(|m| m.is_active() && m.adjust.contrast != 0.)
}
/// The photo reduced for measuring it, without the measured Texture: a user
/// adjustment that depends on the preview size.
fn measured_copy(im: Source<'_>) -> std::borrow::Cow<'_, CameraImage> {
    match im.measured {
        Some(copy) => std::borrow::Cow::Borrowed(copy.as_ref()),
        None => std::borrow::Cow::Owned(super::measurement_source(
            Source::new(im.untextured.unwrap_or(im.image)),
            crate::develop::local_tone::MAP_EDGE,
        )),
    }
}
/// Camera Raw's Contrast pivot for this photo, from its reduced copy rendered as the
/// recipe's profile, white balance and calibration render it, at the camera's
/// exposure: the user's Exposure does not move it (measured on the chart). Dehaze is
/// rendered, the photo's `haze` read at the copy's own pixels.
pub(super) fn contrast_pivot(
    im: Source,
    r: &Recipe,
    camera: Camera,
    measures: &crate::develop::scene_tone::PhotoMeasures,
    haze: Option<Arc<crate::develop::scene_tone::Haze>>,
) -> f32 {
    let small = measured_copy(im);
    // Measured on the SDR rendering, HDR or not, as Camera Raw measures it.
    let mut default = Recipe {
        exposure: 0.,
        whites: 0.,
        blacks: 0.,
        ..r.clone()
    };
    default.hdr.enabled = false;
    let mut lut = CurveSet::new(&default);
    let haze = haze.map(|haze| {
        crate::develop::scene_tone::Dehazing::new(haze, [small.width, small.height], 0.)
    });
    lut.measure(*measures, haze, &default);
    let width = small.width as usize;
    let encoded: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .enumerate()
        .map(|(i, p)| {
            let pos = [(i % width) as f32, (i / width) as f32];
            tone_stage(*p, &default, &lut, camera, pos, None).map(|v| srgb_encode(v.clamp(0., 1.)))
        })
        .collect();
    let pivot = crate::develop::basic_tone::photo_pivot(&crate::develop::basic_tone::blocks(
        &encoded,
        small.width as usize,
        small.height as usize,
    ));
    if small.metadata.is_rendered() {
        crate::develop::basic_tone::rendered_pivot(pivot)
    } else {
        pivot
    }
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
