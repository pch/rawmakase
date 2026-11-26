//! A rendered file (JPEG, PNG, TIFF) as Lightroom develops it: as the scene whose
//! default rendering it is. Camera Raw's default rendering `F` is the scene tone
//! stage's white curve at keys 0 (sensor white, maximum, Whites level and white point
//! all at 1; no black point, as the Embedded profile) applied DNG RGBTone fashion,
//! followed by the ACR3 tone curve applied with PV2012's hue-ratio-preserving tone
//! operator ([`rawmakase_model::camera_profiles::pv2012_tone`]). An SDR rendered file's
//! linear ProPhoto values go through `F⁻¹` once, when it opens, and then render through
//! the raw path like any scene; at default settings that path is `F`, so the file comes
//! back as it is (docs/scene-tone-stage.md, rendered files).
//!
//! An HDR file's master renders from an HDR pseudo-scene of its light in the same way:
//! [`scene_from_light`] inverts `F_hdr`, what the HDR master renders a scene as at a
//! new file's settings, so that master renders the light again. `F_hdr` is the
//! identity from 0.5 up, as Camera Raw renders an HDR file's light as a scene through
//! a linear curve (docs/records/hdr-stage4-design.md).
use crate::camera_data::CameraImage;
use crate::develop::scene_tone::{GlobalTone, rgb_tone};
use rawmakase_model::camera_profiles::{default_tone_at, default_tone_inv_at, pv2012_tone};
use rayon::prelude::*;
use std::sync::OnceLock;

/// log2 scene values `F` is tabulated over, and its samples: 128 per stop.
const FIRST: f32 = -24.;
const LAST: f32 = 8.;
const SAMPLES: usize = 4097;

/// The white curve the default rendering uses. Its black and dark keys (−12) are
/// inert: the white curve reads neither, and without Blacks or a black point nothing
/// else is built from them.
fn white() -> &'static GlobalTone {
    static WHITE: OnceLock<GlobalTone> = OnceLock::new();
    WHITE.get_or_init(|| GlobalTone::new([0.; 4], 0., 0., -12., -12., false))
}

/// `F` on one neutral value: the white curve, then ACR3 as the profile applies it.
#[cfg(test)]
pub(crate) fn default_rendering(s: f32) -> f32 {
    default_tone_at(white().white_curve(s))
}

/// The log2 scene value of the table's `k`th sample.
fn log_at(k: usize) -> f32 {
    FIRST + (LAST - FIRST) * k as f32 / (SAMPLES - 1) as f32
}

/// The S3 white curve tabulated at log2 scene values from `FIRST` to `LAST`, clamped to
/// `[0, 1]` and made non-decreasing.
fn white_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Vec::with_capacity(SAMPLES);
        let mut previous = 0f32;
        for k in 0..SAMPLES {
            previous = previous.max(white().white_curve(log_at(k).exp2()).min(1.));
            out.push(previous);
        }
        out
    })
}

/// Inverse of the S3 white curve on one value in `[0, 1]`: the smallest scene value the
/// white curve takes to `u` (interpolated in log2), linear below the table, and at 1 and
/// above where the curve first reaches 1.
fn white_scene_value(u: f32) -> f32 {
    if u.is_nan() || u <= 0. {
        return 0.;
    }
    let u = u.min(1.);
    let outputs = white_table();
    let k = outputs.partition_point(|&v| v < u);
    if k == 0 {
        return log_at(0).exp2() * u / outputs[0].max(f32::MIN_POSITIVE);
    }
    if k == outputs.len() {
        return log_at(SAMPLES - 1).exp2();
    }
    let (y0, y1) = (outputs[k - 1], outputs[k]);
    let t = if y1 > y0 { (u - y0) / (y1 - y0) } else { 1. };
    (log_at(k - 1) + (log_at(k) - log_at(k - 1)) * t).exp2()
}

/// Camera Raw's default rendering of one linear ProPhoto pseudo-scene pixel: the S3
/// white curve (DNG RGBTone) followed by the S4 ACR3 tone curve (PV2012 tone operator).
/// Exact inverse of [`scene_from_rendered`] on every unclipped pixel.
pub(crate) fn rendered_from_scene(s: [f32; 3]) -> [f32; 3] {
    let s = s.map(|v| if v.is_finite() { v.max(0.) } else { 0. });
    let u = rgb_tone(s, |v| white().white_curve(v));
    pv2012_tone(u, default_tone_at)
}

/// The pseudo-scene of one SDR rendered pixel (linear ProPhoto): inverts Camera Raw's
/// PV2012 S4 ACR3 tone curve (`pv2012_tone` with [`default_tone_inv_at`]) and then the
/// S3 white curve (`rgb_tone` with `white_scene_value`). Values that are not finite are
/// taken as 0.
pub fn scene_from_rendered(p: [f32; 3]) -> [f32; 3] {
    let p = p.map(|v| if v.is_finite() { v.clamp(0., 1.) } else { 0. });
    let u = pv2012_tone(p, default_tone_inv_at);
    rgb_tone(u, white_scene_value)
}

/// A rendered file's image as the scene its default rendering is of.
///
/// An HDR file's master renders from its separate `HdrLight` buffer through
/// [`scene_from_light`]; this only transforms the SDR pixel buffer (`image.pixels`),
/// keeping a gain-map file's SDR base identical to opening the plain SDR image.
pub fn to_scene(mut image: CameraImage) -> CameraImage {
    image
        .pixels
        .par_iter_mut()
        .for_each(|p| *p = scene_from_rendered(*p));
    image
}

/// log2 scene values `F_hdr` is tabulated over below [`HDR_IDENTITY_FROM`], and its
/// samples: 512 per stop.
const HDR_FIRST: f32 = -24.;
const HDR_SAMPLES: usize = 11777;
/// Light from here up is its own scene: `F_hdr` is the identity there.
const HDR_IDENTITY_FROM: f32 = 0.5;

/// `F_hdr`, an HDR file's default rendering on one value: what its HDR master renders
/// a gray of scene value `s` as at a new file's settings. That is the scene tone
/// stage's HDR white curve at Whites 0, the default curve's toe below scene 0.5 and the
/// identity above, then the Embedded profile's curve, linear for an HDR file
/// ([`crate::camera_profiles::CameraProfile::for_hdr_light`]), which renders in HDR as
/// the identity. The toe is within 0.0015 stop of the identity.
pub(crate) fn hdr_default_rendering(s: f32) -> f32 {
    static WHITE: OnceLock<GlobalTone> = OnceLock::new();
    WHITE
        .get_or_init(|| GlobalTone::hdr(0., 0., 0., -12., -12., false))
        .white_curve(s)
}

fn hdr_log_at(k: usize) -> f32 {
    HDR_FIRST + (HDR_IDENTITY_FROM.log2() - HDR_FIRST) * k as f32 / (HDR_SAMPLES - 1) as f32
}

/// `F_hdr` in log2 at log2 scene values from `HDR_FIRST` to 0.5, made non-decreasing.
fn hdr_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut previous = f32::NEG_INFINITY;
        (0..HDR_SAMPLES)
            .map(|k| {
                let y = hdr_default_rendering(hdr_log_at(k).exp2()).max(f32::MIN_POSITIVE);
                previous = previous.max(y.log2());
                previous
            })
            .collect()
    })
}

/// `F_hdr⁻¹` on one value of light (1 = graphics white): the light itself from 0.5 up;
/// below, the smallest scene value `F_hdr` takes to `y`, interpolated in log2 on both
/// axes, and linear below the table.
fn hdr_scene_value(y: f32) -> f32 {
    if y.is_nan() || y <= 0. {
        return 0.;
    }
    if y >= HDR_IDENTITY_FROM {
        return y;
    }
    let outputs = hdr_table();
    let ly = y.log2();
    let k = outputs.partition_point(|&v| v < ly);
    if k == 0 {
        return (HDR_FIRST + ly - outputs[0]).exp2();
    }
    if k == outputs.len() {
        return HDR_IDENTITY_FROM;
    }
    let (y0, y1) = (outputs[k - 1], outputs[k]);
    let t = if y1 > y0 { (ly - y0) / (y1 - y0) } else { 1. };
    (hdr_log_at(k - 1) + (hdr_log_at(k) - hdr_log_at(k - 1)) * t).exp2()
}

/// The pseudo-scene of one pixel of an HDR file's light (linear ProPhoto, 1 = graphics
/// white): `F_hdr⁻¹` DNG RGBTone fashion, so that an HDR master of it at the file's
/// own settings is the light again: exactly from 0.5 up, within 0.0001 stop below. Negative channels are taken as 0, as an HDR
/// master never renders them; values that are not finite as 0.
pub fn scene_from_light(p: [f32; 3]) -> [f32; 3] {
    let p = p.map(|v| if v.is_finite() { v.max(0.) } else { 0. });
    rgb_tone(p, hdr_scene_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `F(F⁻¹(y)) = y`, finely enough for 16-bit levels, from deep shadows to white.
    #[test]
    fn the_default_rendering_of_the_pseudo_scene_is_the_file() {
        for k in 0..=4096 {
            let y = if k == 0 {
                1.
            } else {
                (-16. * k as f32 / 4096.).exp2()
            };
            let back = default_rendering(white_scene_value(default_tone_inv_at(y)));
            assert!((back - y).abs() <= 1e-4_f32.max(y * 1e-3), "{y}: {back}");
        }
        // Full 3D RGB round-trip across saturated sRGB and Display P3 colors.
        for rgb in [
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1., 1., 0.],
            [0., 1., 1.],
            [1., 0., 1.],
            [0.8, 0.2, 0.01],
            [0.05, 0.7, 0.4],
            [0.3, 0.05, 0.9],
        ] {
            let pro =
                rawmakase_model::color::mul(rawmakase_model::camera_profiles::RGB_TO_PRO, rgb);
            let back = rendered_from_scene(scene_from_rendered(pro));
            for c in 0..3 {
                assert!(
                    (back[c] - pro[c]).abs() <= 2e-4,
                    "{rgb:?} -> pro {pro:?} -> back {back:?}"
                );
            }
        }
        // Extreme out-of-Rec.2020 ProPhoto corners fall outside PV2012 ACR3's [0, 1]^3
        // range and clamp identically to Camera Raw 18.7 at Exposure 0:
        // linear ProPhoto [1.0, 0.2811, 0.0] -> [0.9991, 0.3077, 0.0374]
        // linear ProPhoto [1.0, 0.4974, 0.0] -> [0.9992, 0.5046, 0.0151].
        for (input, cr_default) in [
            ([1.0, 0.2811, 0.0], [0.9991, 0.3077, 0.0374]),
            ([1.0, 0.4974, 0.0], [0.9992, 0.5046, 0.0151]),
        ] {
            let back = rendered_from_scene(scene_from_rendered(input));
            for c in 0..3 {
                assert!(
                    (back[c] - cr_default[c]).abs() <= 1.5e-3,
                    "ProPhoto corner {input:?}: got {back:?}, Camera Raw 18.7 {cr_default:?}"
                );
            }
        }
    }

    /// `F_hdr(F_hdr⁻¹(y)) = y`: within 0.0001 stop from deep shadows to 0.5, and exactly
    /// above, as far as light goes.
    #[test]
    fn the_hdr_default_rendering_of_the_hdr_pseudo_scene_is_the_light() {
        let mut worst = 0f32;
        for k in 0..=8192 {
            let y = (-16. + 15. * k as f32 / 8192.).exp2();
            let back = hdr_default_rendering(hdr_scene_value(y));
            worst = worst.max((back / y).log2().abs());
        }
        assert!(worst < 1e-4, "{worst} stop");
        for y in [0.5, 0.5001, 1., 7.3, 49.26, 1e4, f32::MAX] {
            assert_eq!(hdr_default_rendering(hdr_scene_value(y)), y, "{y}");
        }
        let mut previous = 0.;
        for k in 0..=65535 {
            let s = hdr_scene_value(k as f32 / 1024.);
            assert!(s >= previous, "{k}: {s} < {previous}");
            previous = s;
        }
        assert_eq!(scene_from_light([-1., f32::NAN, 0.]), [0.; 3]);
    }

    #[test]
    fn the_pseudo_scene_is_monotone() {
        let mut previous = 0.;
        for k in 0..=65535 {
            let s = white_scene_value(default_tone_inv_at(k as f32 / 65535.));
            assert!(s >= previous, "{k}: {s} < {previous}");
            previous = s;
        }
    }

    #[test]
    fn black_white_and_broken_values_stay_finite() {
        assert_eq!(scene_from_rendered([0.; 3]), [0.; 3]);
        assert_eq!(scene_from_rendered([f32::NAN, -1., f32::INFINITY]), [0.; 3]);
        let white = scene_from_rendered([1.; 3]);
        assert!(white.iter().all(|v| v.is_finite() && *v > 0.5), "{white:?}");
        assert!(white[0] == white[1] && white[1] == white[2], "{white:?}");
        // White is where the default rendering first reaches 1.
        assert!(default_rendering(white[0]) >= 1. - 1e-6);
        assert!(default_rendering(white[0] * 0.98) < 1.);
    }

    /// Below the table's first output the pseudo-scene is linear, and meets the table
    /// there without a step.
    #[test]
    fn below_the_table_the_pseudo_scene_is_linear() {
        let first = white_table()[0];
        assert!(first > 0., "{first}");
        let at = white_scene_value(first);
        assert!((at / FIRST.exp2() - 1.).abs() < 1e-4, "{at}");
        for y in [first * 0.999, first * 0.5, first * 1e-3] {
            let ratio = white_scene_value(y) / y;
            assert!((ratio * first / at - 1.).abs() < 1e-4, "{y}: {ratio}");
        }
        let above = white_scene_value(first * 1.001);
        assert!(above > at && above < at * 1.01, "{above}");
    }

    /// The table reaches white within its range, so values at and above white all
    /// take the scene value where `F` first reaches 1, short of the table's end.
    #[test]
    fn white_and_above_take_the_scene_white() {
        let outputs = white_table();
        assert_eq!(outputs[SAMPLES - 1], 1.);
        let white = white_scene_value(1.);
        assert!(white < LAST.exp2(), "{white}");
        for y in [1. + 1e-6, 1.5, 100., f32::MAX] {
            assert_eq!(white_scene_value(y), white, "{y}");
        }
        assert!(white_scene_value(1. - 1e-4) < white);
    }

    /// RGBTone: the middle channel keeps its place between the others, so hue holds.
    #[test]
    fn colours_keep_their_hue() {
        for p in [[0.6, 0.2, 0.05], [0.02, 0.5, 0.9], [0.95, 0.94, 0.1]] {
            let s = scene_from_rendered(p);
            let order = |q: [f32; 3]| {
                let mut i = [0, 1, 2];
                i.sort_by(|a, b| q[*a].total_cmp(&q[*b]));
                i
            };
            assert_eq!(order(p), order(s), "{p:?} -> {s:?}");
            let [l, m, h] = order(p);
            let ratio = |q: [f32; 3]| (q[m] - q[l]) / (q[h] - q[l]);
            assert!((ratio(p) - ratio(s)).abs() < 1e-5, "{p:?} -> {s:?}");
        }
    }
}
