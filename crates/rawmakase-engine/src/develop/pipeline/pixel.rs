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
    let scene = exposure_stage(scene_color(p, r, lut, matrix, local), lut, local);
    if lut.output == PixelOutput::SceneInput {
        return scene;
    }
    let scene = if lut.output == PixelOutput::AutoBasis {
        auto_basis(scene, r)
    } else {
        scene_stage(scene, r, lut, pos, local)
    };
    if lut.output == PixelOutput::SceneOutput {
        return scene;
    }
    let rgb = profile_stage(scene, r);
    color_stage(rgb, r, lut, local.map(|l| l.delta))
}
/// Camera sample to linear display RGB after the camera profile's look and tone curve,
/// through the scene stages S1–S4 (see `docs/scene-tone-stage.md`).
pub(super) fn tone_stage(
    p: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
    pos: [f32; 2],
    local: Option<Local>,
) -> [f32; 3] {
    let scene = exposure_stage(scene_color(p, r, lut, matrix, local), lut, local);
    profile_stage(scene_stage(scene, r, lut, pos, local), r)
}
/// S1: a camera sample's scene colour: white balance (a mask's too), the profile's
/// matrix and HueSatMap, and Camera Calibration. Linear, sRGB primaries.
pub(super) fn scene_color(
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
    lut.calibration.apply(color)
}
/// S2: Exposure (a mask's and its colour too), in linear Rec.2020. Returns linear
/// ProPhoto RGB, unclamped: the scene tone stage's input. The black point is the scene
/// tone stage's, as Camera Raw's does not follow the camera's baseline exposure.
pub(super) fn exposure_stage(color: [f32; 3], lut: &CurveSet, local: Option<Local>) -> [f32; 3] {
    let exposure = local.map_or(0., |l| l.delta[slot::EXPOSURE]);
    let mut rgb = mul(TO_2020, color).map(|v| v * lut.exposure_gain * exposure.exp2());
    if let Some(l) = local {
        for (c, v) in rgb.iter_mut().enumerate() {
            *v *= l.delta[slot::COLOR + c].exp2();
        }
    }
    mul(crate::camera_profiles::RGB_TO_PRO, mul(FROM_2020, rgb))
}
/// S3: the scene tone stage, on linear ProPhoto RGB, unclamped: Shadows, Highlights and
/// Clarity as a gain from the photo's map at sample position `pos`, then Dehaze (with the
/// photo's haze at `pos`) and the
/// global curves.
pub(super) fn scene_stage(
    pro: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    pos: [f32; 2],
    local: Option<Local>,
) -> [f32; 3] {
    let pro = match &lut.local {
        Some(map) => {
            let lum = crate::develop::scene_tone::luminance(pro);
            let sliders = local
                .filter(|l| local::uses(l.delta, &[slot::SHADOWS, slot::HIGHLIGHTS, slot::CLARITY]))
                .map(|l| {
                    [
                        r.shadows + l.delta[slot::SHADOWS],
                        r.highlights + l.delta[slot::HIGHLIGHTS],
                        r.effects.clarity + l.delta[slot::CLARITY],
                    ]
                });
            let gain = match sliders {
                Some(sliders) => map.gain_with(pos[0], pos[1], lum, sliders),
                None => map.gain(pos[0], pos[1], lum),
            };
            pro.map(|v| v * gain)
        }
        None => pro,
    };
    lut.scene.apply(pro, pos, local.map(|l| l.delta))
}
/// In place of S3 for Auto tone's measurement (`PixelOutput::AutoBasis`): the DNG SDK's
/// exposure ramp at its default Shadows, with its black at 0.0015 × 2^exposure (none
/// under a profile whose DefaultBlackRender is None), as RAWmakase rendered the default
/// before the scene tone stage.
fn auto_basis(pro: [f32; 3], r: &Recipe) -> [f32; 3] {
    let black = match r.profile.as_ref().map(|p| p.black_render()) {
        Some(crate::camera_profiles::BlackRender::None) => 0.,
        _ => 0.0015 * (r.exposure + r.camera_exposure).exp2(),
    };
    let black = black.clamp(0., 0.5);
    let slope = 1. / (1. - black);
    let radius = (0.5 * black).min(1. / 16. / slope);
    let q = if radius > 0. {
        slope / (4. * radius)
    } else {
        0.
    };
    // The ramp ran on Rec.2020 channels.
    let wide = mul(TO_2020, mul(crate::camera_profiles::PRO_TO_RGB, pro)).map(|x| {
        if x <= black - radius {
            0.
        } else if x >= black + radius {
            (x - black) * slope
        } else {
            q * (x - (black - radius)).powi(2)
        }
    });
    mul(crate::camera_profiles::RGB_TO_PRO, mul(FROM_2020, wide))
}
/// S4: the profile's look tables and tone curve, from linear ProPhoto RGB to linear
/// sRGB-primaries display RGB. Without a profile, a scene-referred shoulder anchored
/// at 18% middle gray, on luminance.
pub(super) fn profile_stage(pro: [f32; 3], r: &Recipe) -> [f32; 3] {
    match &r.profile {
        Some(profile) => profile.finish(pro),
        None => {
            let rgb = mul(crate::camera_profiles::PRO_TO_RGB, pro);
            let y = crate::color::luminance(rgb).max(1e-8);
            let x = y.max(0.);
            rgb.map(|v| v * (x / (x + 0.82)) / y)
        }
    }
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
        PixelOutput::Display | PixelOutput::ColorInput | PixelOutput::AutoBasis => {}
        PixelOutput::SceneInput | PixelOutput::SceneOutput => {
            unreachable!("process_pixel returns the scene stage's taps")
        }
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
    /// Linear ProPhoto RGB entering the scene tone stage (S3): after white balance,
    /// the profile's matrix and HueSatMap, Calibration, Exposure and the black point.
    SceneInput,
    /// Linear ProPhoto RGB leaving the scene tone stage, before the profile's look and
    /// tone curve: where Camera Raw's renders of a linear-profile DNG compare.
    SceneOutput,
    /// The finished colour with the DNG exposure ramp in place of the scene tone stage:
    /// the render Auto tone's fit measured (docs/tone-controls.md#auto), kept so Auto
    /// gives the same results.
    AutoBasis,
}
