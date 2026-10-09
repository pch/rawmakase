//! The per-pixel stages: tone, the colour mixer and colour grading, then the finish into sRGB.
use super::*;
use crate::develop::effects::EffectsRendering;

/// A pixel's mask adjustments and the render's constants for them.
#[derive(Clone, Copy)]
pub(crate) struct Local<'a> {
    pub(crate) delta: &'a LocalDelta,
    pub(crate) math: &'a LocalMath,
}
pub(super) fn process_pixel(
    p: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
    pos: [f32; 2],
    local: Option<Local>,
) -> [f32; 3] {
    let rgb = tone_stage(p, r, lut, matrix, local);
    let rgb = match &lut.local {
        Some(map) => {
            let sliders = local
                .filter(|l| local::uses(l.delta, &[slot::SHADOWS, slot::HIGHLIGHTS]))
                .map(|l| {
                    [
                        r.shadows + l.delta[slot::SHADOWS],
                        r.highlights + l.delta[slot::HIGHLIGHTS],
                    ]
                });
            let gain = match sliders {
                Some(sliders) => map.gain_with(pos[0], pos[1], rgb, sliders),
                None => map.gain(pos[0], pos[1], rgb),
            };
            rgb.map(|v| v * gain)
        }
        None => rgb,
    };
    color_stage(rgb, r, lut, local.map(|l| l.delta))
}
/// Camera sample to linear display RGB after the camera profile's tone curve.
pub(super) fn tone_stage(
    p: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
    local: Option<Local>,
) -> [f32; 3] {
    let wb = local.map_or([1.; 3], |l| l.math.white_balance_gain(l.delta));
    let p = std::array::from_fn(|c| p[c] * r.wb[c] * wb[c]);
    let color = r.profile.as_ref().map_or_else(
        || mul(matrix, p),
        |profile| profile.camera_color(p, matrix, r.temperature),
    );
    let color = lut.calibration.apply(color);
    let exposure = local.map_or(0., |l| l.delta[slot::EXPOSURE]);
    let mut rgb = mul(TO_2020, color).map(|v| v * lut.exposure_gain * exposure.exp2());
    if let Some(l) = local {
        for (c, v) in rgb.iter_mut().enumerate() {
            *v *= l.delta[slot::COLOR + c].exp2();
        }
    }
    if let Some(ramp) = &lut.black_ramp {
        if exposure != 0. {
            // The ramp's black point follows exposure, as for the global slider.
            let ramp = ExposureRamp::new(
                default_black(r) * (r.exposure + r.camera_exposure + exposure).exp2(),
            );
            rgb = rgb.map(|v| ramp.eval(v));
        } else {
            rgb = rgb.map(|v| ramp.eval(v));
        }
    }
    // Dehaze, Whites and Blacks render as measured curves in `apply_reference_curves`,
    // Shadows and Highlights with the local operator in `local_tone.rs`.
    let y = luma(rgb).max(1e-8);
    let mapped = if r.profile.is_some() {
        y
    } else {
        // Scene-referred shoulder anchored at 18% middle gray. No per-channel clipping.
        let x = y.max(0.);
        x / (x + 0.82)
    };
    rgb = rgb.map(|v| v * mapped / y);
    let rgb = mul(FROM_2020, rgb);
    r.profile.as_ref().map_or(rgb, |p| p.finish(rgb))
}
/// The tone curves, the color mixer and Point Color: linear display RGB as Point
/// Color leaves it, and Visualize Range's selection.
fn mixer_stage(
    rgb: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> crate::develop::point_color::Rendered {
    let sampled = |color| crate::develop::point_color::Rendered {
        color,
        selection: None,
    };
    if lut.output == PixelOutput::CurveInput {
        let [red, green, blue] = curve_input(rgb, r, lut, local);
        return sampled([0.299 * red + 0.587 * green + 0.114 * blue; 3]);
    }
    let rgb = apply_reference_curves(rgb, r, lut, local);
    // Lightroom grades after the tone curves: a faded point curve changes which tones
    // count as shadows. The measured color mixer, after the tone curves, matches
    // Lightroom references with point curves.
    if lut.output == PixelOutput::MixerInput {
        return sampled(mul(crate::camera_profiles::RGB_TO_PRO, rgb));
    }
    let rgb = lut.mixer.as_ref().map_or(rgb, |m| m.apply(rgb));
    // Point Color works where the mixer does, in HSV of linear ProPhoto RGB.
    match &lut.point_colors {
        Some(p) => {
            let out = p.render_prophoto(mul(crate::camera_profiles::RGB_TO_PRO, rgb));
            crate::develop::point_color::Rendered {
                color: mul(crate::camera_profiles::PRO_TO_RGB, out.color),
                selection: out.selection,
            }
        }
        None => crate::develop::point_color::Rendered {
            color: rgb,
            selection: None,
        },
    }
}
/// Basic curves, point curves, color controls and output encoding.
pub(super) fn color_stage(
    rgb: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> [f32; 3] {
    let mixed = mixer_stage(rgb, r, lut, local);
    let rgb = mixed.color;
    match lut.output {
        PixelOutput::PointColor => return mul(crate::camera_profiles::RGB_TO_PRO, rgb),
        PixelOutput::CurveInput | PixelOutput::MixerInput => return rgb,
        PixelOutput::Display | PixelOutput::ColorInput => {}
    }
    // A look's RGB table: after the colour mixer, before colour grading, as Camera
    // Raw 18.7 applies it (also after the user's tone curves and Saturation).
    let rgb = lut.rgb_table.as_ref().map_or(rgb, |t| t.apply(rgb));
    let rgb = lut.grade.as_ref().map_or(rgb, |g| g.apply(rgb));
    let mut lab = srgb_to_lab(rgb);
    if let Some(d) = local {
        lab = local::hue_saturation(d, lab);
    }
    if lut.output == PixelOutput::ColorInput {
        return lab;
    }
    if lut.color_adjustments {
        // HSL, Saturation and Vibrance render in the measured color mixer; Defringe
        // and the black & white mix work here, on the color's Oklab hue.
        let chroma = lab[1].hypot(lab[2]);
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        let angle = hue * std::f32::consts::TAU;
        lab[0] = lab[0].clamp(0., 1.);
        lab[1] = angle.cos() * chroma;
        lab[2] = angle.sin() * chroma;
        lab = r.effects.defringe_color(lab, hue);
        // `gray_grid` is there for black & white renders.
        if let Some(grid) = &lut.gray_grid {
            // The gray's Oklab lightness is the cube root of its luminance.
            lab[0] = crate::develop::black_white::gray(grid, lab_to_srgb(lab))
                .cbrt()
                .clamp(0., 1.);
            lab[1] = 0.;
            lab[2] = 0.;
        }
    } else {
        // Identity color controls need no hue angle or trigonometry.
        lab[0] = lab[0].clamp(0., 1.);
    }
    let out = finish_color(lab);
    // Visualize Range grays what the swatch leaves out after every color control, so
    // none of them tints it.
    mixed
        .selection
        .map_or(out, |w| crate::develop::point_color::visualize(out, w))
}
/// The colour stage's end, after the colour controls and Defringe: each channel
/// clipped into sRGB on its own, as Camera Raw's conversion does
/// (docs/color-pipeline.md#out-of-gamut-colors). Returns encoded sRGB.
pub(super) fn finish_color(lab: [f32; 3]) -> [f32; 3] {
    lab_to_srgb(lab).map(|v| srgb_encode(v.clamp(0., 1.)).clamp(0., 1.))
}

/// What the per-pixel stage hands back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PixelOutput {
    /// The finished, encoded color.
    #[default]
    Display,
    /// Linear ProPhoto RGB after the swatches already there: what Point Color's
    /// dropper samples.
    PointColor,
    /// The parametric curve's input, as the luma of the three channels it curves
    /// (Rec. 601 weights, as Refine Saturation's): what the Tone Curve's Targeted
    /// Adjustment Tool samples, in every channel.
    CurveInput,
    /// Linear ProPhoto RGB where the color mixer works, after the tone curves: what
    /// the Color Mixer's Targeted Adjustment Tool samples.
    MixerInput,
    /// Oklab where the color controls and the black & white mix take a color's hue
    /// to weigh their bands.
    ColorInput,
}
