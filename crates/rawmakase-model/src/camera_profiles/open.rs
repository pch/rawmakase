//! RAWmakase's own camera profiles, available for every camera with a colour matrix.
//! They need no imported files, so presets and new photos render the same on every
//! system. Everything here is our own work, released with the rest of RAWmakase.
//!
//! - **RAWmakase Standard** is the camera matrix with the DNG default tone curve: the
//!   neutral rendering `CameraProfile::camera_matrix_default` already gives.
//! - **RAWmakase Color** layers a look on Standard, the way Adobe Color layers one on
//!   Adobe Standard: a gentle contrast curve and a few smooth hue, saturation and
//!   brightness shifts, generated from the parameters below rather than stored as a
//!   table.
use super::{CameraProfile, Table, enhanced::Enhanced};
use crate::{
    camera_data::Metadata,
    color::curve::{CurveLut, ToneCurve},
};

pub const STANDARD: &str = "RAWmakase Standard";
pub const COLOR: &str = "RAWmakase Color";
const COPYRIGHT: &str = "RAWmakase contributors, MIT licence";
/// Identifies the Color look in saved recipes; change it when the look changes.
const COLOR_UUID: &str = "4BDC01C1BFEE4C4F9B18CEBC8B5ADBAD";

/// A smooth adjustment around one hue. Hues are in degrees of linear ProPhoto RGB,
/// the space the look table works in: red ≈ 10, skin ≈ 30, yellow ≈ 57,
/// foliage ≈ 90, aqua ≈ 200, sky ≈ 235, purple ≈ 260, magenta ≈ 320.
struct Band {
    hue: f32,
    /// Half-width of the raised-cosine window, in degrees.
    width: f32,
    /// Hue rotation in degrees at the band's centre.
    shift: f32,
    /// Saturation and brightness multipliers at the band's centre.
    saturation: f32,
    value: f32,
}

const COLOR_BANDS: &[Band] = &[
    // Reds a touch richer.
    Band {
        hue: 8.,
        width: 22.,
        shift: 0.,
        saturation: 1.04,
        value: 1.,
    },
    // Skin: slightly away from red and a little softer.
    Band {
        hue: 31.,
        width: 20.,
        shift: 1.5,
        saturation: 0.97,
        value: 1.01,
    },
    // Yellows warmer.
    Band {
        hue: 57.,
        width: 20.,
        shift: -2.5,
        saturation: 1.03,
        value: 1.,
    },
    // Foliage toward yellow and a little quieter, so greens don't turn neon.
    Band {
        hue: 92.,
        width: 30.,
        shift: -3.,
        saturation: 0.96,
        value: 1.,
    },
    // Aqua toward blue.
    Band {
        hue: 200.,
        width: 22.,
        shift: 3.,
        saturation: 1.02,
        value: 1.,
    },
    // Sky: away from purple, deeper and a little richer.
    Band {
        hue: 238.,
        width: 26.,
        shift: -3.,
        saturation: 1.05,
        value: 0.97,
    },
];
/// Highlights lose up to this share of their saturation near white, so bright
/// colours roll off instead of clipping to a flat hue.
const COLOR_HIGHLIGHT_ROLLOFF: f32 = 0.1;
/// Encoded-sRGB points of the Color look's curve: a mild S around mid grey.
const COLOR_CURVE: &[[f32; 2]] = &[
    [0., 0.],
    [0.25, 0.237],
    [0.5, 0.505],
    [0.75, 0.765],
    [1., 1.],
];
const DIMS: [usize; 3] = [72, 8, 8];

/// RAWmakase Standard for this camera, or None without a usable colour matrix.
pub fn standard(m: &Metadata) -> Option<CameraProfile> {
    let mut p = CameraProfile::camera_matrix_default(m).or_else(|| {
        // LibRaw reports no matrix for some DNGs; use the D65 matrix the DNG's own
        // profile carries.
        let embedded = super::builtin(m)?;
        let mut with_matrix = m.clone();
        with_matrix.cam_xyz = embedded.color_matrix(6504.)?;
        CameraProfile::camera_matrix_default(&with_matrix)
    })?;
    p.name = STANDARD.into();
    p.copyright = COPYRIGHT.into();
    Some(p)
}

/// RAWmakase Color for this camera, or None without a usable colour matrix.
pub fn color(m: &Metadata) -> Option<CameraProfile> {
    let base = standard(m)?;
    let mut p = base.clone();
    p.name = COLOR.into();
    p.enhanced = Some(Enhanced {
        uuid: COLOR_UUID.into(),
        base_name: base.name,
        highlights: 0.,
        shadows: 0.,
        clarity: 0.,
        contrast: 0.,
        blacks: 0.,
        monochrome: false,
        amount: None,
        table: Some(color_table()),
        rgb: None,
        settings: Default::default(),
        curve: Box::new(CurveLut::new(&ToneCurve {
            points: COLOR_CURVE.to_vec(),
            ..Default::default()
        })),
    });
    p.validate().ok()?;
    Some(p)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// The Color look's hue/saturation/value table (DNG HueSatMap layout: hue shift in
/// degrees, saturation and value scales), indexed by encoded value.
fn color_table() -> Table {
    let [nh, ns, nv] = DIMS;
    let mut data = vec![[0., 1., 1.]; nh * ns * nv];
    for iz in 0..nv {
        let value = iz as f32 / (nv - 1) as f32;
        let rolloff = 1. - COLOR_HIGHLIGHT_ROLLOFF * smoothstep(0.7, 1., value);
        for ix in 0..nh {
            let hue = ix as f32 * 360. / nh as f32;
            let mut shift = 0.;
            let mut saturation = 1.;
            let mut brightness = 1.;
            for band in COLOR_BANDS {
                let d = (hue - band.hue + 180.).rem_euclid(360.) - 180.;
                if d.abs() >= band.width {
                    continue;
                }
                let w = 0.5 + 0.5 * (std::f32::consts::PI * d / band.width).cos();
                shift += band.shift * w;
                saturation *= 1. + (band.saturation - 1.) * w;
                brightness *= 1. + (band.value - 1.) * w;
            }
            for iy in 0..ns {
                // Neutrals stay neutral: the look fades in with saturation.
                let s = smoothstep(0., 0.35, iy as f32 / (ns - 1) as f32);
                data[(iz * nh + ix) * ns + iy] = [
                    shift * s,
                    (1. + (saturation - 1.) * s) * rolloff,
                    1. + (brightness - 1.) * s,
                ];
            }
        }
    }
    Table {
        dims: DIMS,
        data,
        srgb: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
    fn camera() -> Metadata {
        Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            wb: [2.0198677, 1., 1.8874172],
            cam_xyz: [
                [1.1434, -0.4948, -0.121],
                [-0.3746, 1.2042, 0.1903],
                [-0.0666, 0.1479, 0.5235],
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_profile_for_another_camera_is_a_typed_error_with_the_same_message() {
        let profile = standard(&camera()).unwrap();
        let other = Metadata {
            make: "Canon".into(),
            model: "EOS R5".into(),
            ..camera()
        };
        let error = profile.ensure_camera(&other).unwrap_err();
        let mismatch = error.downcast_ref::<super::super::OtherCamera>().unwrap();
        assert_eq!(mismatch.photo, "Canon EOS R5");
        assert_eq!(
            error.to_string(),
            format!("Profile belongs to {}, not Canon EOS R5", profile.camera)
        );
    }

    #[test]
    fn standard_is_the_camera_matrix_default_by_name() {
        let m = camera();
        let profile = standard(&m).unwrap();
        let mut matrix = CameraProfile::camera_matrix_default(&m).unwrap();
        matrix.name = STANDARD.into();
        matrix.copyright = COPYRIGHT.into();
        assert_eq!(profile, matrix);
        profile.ensure_camera(&m).unwrap();
        assert!(standard(&Metadata::default()).is_none());
        assert!(color(&Metadata::default()).is_none());
    }

    /// The camera profile the X100F corpus chart embeds, as a DCP.
    fn x100f_dcp() -> std::sync::Arc<[u8]> {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/charts/fujifilm-x100f-d65.dng");
        crate::dng::read(&chart)
            .and_then(|dng| dng.profile)
            .expect("the chart embeds a profile")
            .into()
    }

    #[test]
    fn dngs_without_a_libraw_matrix_use_their_own_profile_matrix() {
        let dng = Metadata {
            cam_xyz: [[0.; 3]; 3],
            embedded_dcp: Some(x100f_dcp()),
            ..camera()
        };
        let embedded = super::super::builtin(&dng).unwrap();
        let with_its_matrix = Metadata {
            cam_xyz: embedded.color_matrix(6504.).unwrap(),
            ..camera()
        };
        assert_eq!(standard(&dng), standard(&with_its_matrix));
        assert!(color(&dng).is_some());
    }

    #[test]
    fn color_keeps_neutrals_and_black_and_white() {
        let m = camera();
        let color = super::color(&m).unwrap();
        color.validate().unwrap();
        color.ensure_camera(&m).unwrap();
        assert_eq!(color.enhanced.as_ref().unwrap().base_name, STANDARD);
        for v in [0., 0.05, 0.18, 0.5, 0.9] {
            let out = color.finish([v; 3]);
            assert!(
                (out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4,
                "{v}: {out:?}"
            );
        }
        let black = color.finish([0.; 3]);
        assert!(black.iter().all(|c| c.abs() < 1e-4), "{black:?}");
        let white = color.finish([1.; 3]);
        assert!(white.iter().all(|c| (c - 1.).abs() < 1e-3), "{white:?}");
    }

    #[test]
    fn color_look_is_gentle() {
        let m = camera();
        let (standard, color) = (super::standard(&m).unwrap(), super::color(&m).unwrap());
        // Mid-tone colours move, but stay close to Standard.
        for rgb in [
            [0.45, 0.2, 0.12],
            [0.1, 0.3, 0.08],
            [0.08, 0.15, 0.45],
            [0.5, 0.45, 0.1],
        ] {
            let a = standard.finish(rgb);
            let b = color.finish(rgb);
            let diff = (0..3).map(|c| (a[c] - b[c]).abs()).fold(0., f32::max);
            assert!(diff > 1e-4 && diff < 0.08, "{rgb:?}: {a:?} vs {b:?}");
        }
        // Contrast: shadows a little darker, highlights a little brighter.
        let dark = color.finish([0.03; 3])[1];
        let light = color.finish([0.5; 3])[1];
        assert!(dark < standard.finish([0.03; 3])[1]);
        assert!(light > standard.finish([0.5; 3])[1]);
    }

    #[test]
    fn new_photos_default_to_rawmakase_color_without_adobe_profiles() {
        use crate::model::recipe::Recipe;
        use std::sync::Arc;
        let m = camera();
        let profiles: Vec<_> = [standard(&m), color(&m)]
            .into_iter()
            .flatten()
            .map(Arc::new)
            .collect();
        let r = Recipe::with_profiles(&m, &profiles);
        assert_eq!(r.profile.as_ref().unwrap().name, COLOR);
        // Imported Adobe Color wins, as in Lightroom.
        let mut with_adobe = profiles.clone();
        let mut adobe = color(&m).unwrap();
        adobe.name = "Adobe Color".into();
        with_adobe.push(Arc::new(adobe));
        let r = Recipe::with_profiles(&m, &with_adobe);
        assert_eq!(r.profile.as_ref().unwrap().name, "Adobe Color");
        // A DNG keeps the profile it embeds.
        let dng = Metadata {
            embedded_dcp: Some(x100f_dcp()),
            ..camera()
        };
        let embedded = super::super::builtin(&dng).unwrap();
        let r = Recipe::with_profiles(&dng, &profiles);
        assert_eq!(r.profile.as_ref().unwrap().name, embedded.name);
    }
}
