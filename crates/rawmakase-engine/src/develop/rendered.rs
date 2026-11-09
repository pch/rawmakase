//! A rendered file (JPEG, PNG, TIFF) as Lightroom develops it: as the scene whose
//! default rendering it is. Camera Raw's default rendering `F` is the scene tone
//! stage's white curve at keys 0 (sensor white, maximum, Whites level and white point
//! all at 1; no black point, as the Embedded profile) followed by the ACR3 tone curve,
//! each applied DNG RGBTone fashion. A rendered file's linear ProPhoto values go
//! through `F⁻¹` once, when it opens, and then render through the raw path like any
//! scene; at default settings that path is `F`, so the file comes back as it is
//! (docs/scene-tone-stage.md, rendered files).
//!
//! An HDR file's master renders from an HDR pseudo-scene of its light in the same way:
//! [`scene_from_light`] inverts `F_hdr`, what the HDR master renders a scene as at a
//! new file's settings, so that master renders the light again. `F_hdr` is the
//! identity from 0.5 up, as Camera Raw renders an HDR file's light as a scene through
//! a linear curve (docs/records/hdr-stage4-design.md).
use crate::camera_data::CameraImage;
use crate::develop::scene_tone::{GlobalTone, rgb_tone};
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

/// `F` on one value: the white curve, then ACR3 as the profile applies it.
pub(crate) fn default_rendering(s: f32) -> f32 {
    rawmakase_model::camera_profiles::default_tone_at(white().white_curve(s))
}

/// The log2 scene value of the table's `k`th sample.
fn log_at(k: usize) -> f32 {
    FIRST + (LAST - FIRST) * k as f32 / (SAMPLES - 1) as f32
}

/// `F` tabulated at log2 scene values from `FIRST` to `LAST`, made non-decreasing.
fn table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Vec::with_capacity(SAMPLES);
        let mut previous = 0f32;
        for k in 0..SAMPLES {
            previous = previous.max(default_rendering(log_at(k).exp2()));
            out.push(previous);
        }
        out
    })
}

/// `F⁻¹` on one value: the smallest scene value `F` takes to `y` (interpolated in
/// log2), linear below the table, and at 1 and above where `F` first reaches 1.
fn scene_value(y: f32) -> f32 {
    if y.is_nan() || y <= 0. {
        return 0.;
    }
    let y = y.min(1.);
    let outputs = table();
    let k = outputs.partition_point(|&v| v < y);
    if k == 0 {
        return log_at(0).exp2() * y / outputs[0].max(f32::MIN_POSITIVE);
    }
    if k == outputs.len() {
        return log_at(SAMPLES - 1).exp2();
    }
    let (y0, y1) = (outputs[k - 1], outputs[k]);
    let t = if y1 > y0 { (y - y0) / (y1 - y0) } else { 1. };
    (log_at(k - 1) + (log_at(k) - log_at(k - 1)) * t).exp2()
}

/// The pseudo-scene of one rendered pixel (linear ProPhoto): `F⁻¹` DNG RGBTone
/// fashion, which inverts the white curve and ACR3 applied that way in turn.
/// Values that are not finite are taken as 0.
pub fn scene_from_rendered(p: [f32; 3]) -> [f32; 3] {
    let p = p.map(|v| if v.is_finite() { v } else { 0. });
    rgb_tone(p, scene_value)
}

/// A rendered file's image as the scene its default rendering is of.
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
        .get_or_init(|| GlobalTone::hdr(0., 0., -12., -12., false))
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
            let back = default_rendering(scene_value(y));
            assert!((back - y).abs() <= 1e-4_f32.max(y * 1e-3), "{y}: {back}");
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
            let s = scene_value(k as f32 / 65535.);
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
        let first = table()[0];
        assert!(first > 0., "{first}");
        let at = scene_value(first);
        assert!((at / FIRST.exp2() - 1.).abs() < 1e-4, "{at}");
        for y in [first * 0.999, first * 0.5, first * 1e-3] {
            let ratio = scene_value(y) / y;
            assert!((ratio * first / at - 1.).abs() < 1e-4, "{y}: {ratio}");
        }
        let above = scene_value(first * 1.001);
        assert!(above > at && above < at * 1.01, "{above}");
    }

    /// The table reaches white within its range, so values at and above white all
    /// take the scene value where `F` first reaches 1, short of the table's end.
    #[test]
    fn white_and_above_take_the_scene_white() {
        let outputs = table();
        assert_eq!(outputs[SAMPLES - 1], 1.);
        let white = scene_value(1.);
        assert!(white < LAST.exp2(), "{white}");
        for y in [1. + 1e-6, 1.5, 100., f32::MAX] {
            assert_eq!(scene_value(y), white, "{y}");
        }
        assert!(scene_value(1. - 1e-4) < white);
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
