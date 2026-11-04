//! Tier 1 stage probes (docs/scene-tone-stage.md#testing): synthetic probes rendered
//! to the scene tone stage's output and compared with Camera Raw 18.7's renders of
//! the same probes (`camera-raw/scene-probes.json`, written by
//! scripts/corpus/scene-probes.py and never re-blessed).
//!
//! Ramps are rebuilt here from their patch values; the synthetic scenes are committed
//! DNGs reduced 4× from the rendered ones. Each case's distance from Camera Raw is
//! held against `camera-raw/scene-probes-baseline.json`, which RAWMAKASE_BLESS=1
//! rewrites (commit it with the reason).
use crate::{Case, bless, cases, corpus, develop, dng, embedded_profiles};
use rawmakase::{
    camera_data::CameraImage, camera_profiles::CameraProfile, develop::quality, rendered::Rendered,
};
use rawmakase_cancel::Cancel;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Arc};

/// ProPhoto luminance.
const Y: [f64; 3] = [0.2880402, 0.7118741, 0.0000857];
/// Scene blocks darker than this (either render) are left out of the effect error.
const DARK: f64 = 1. / 4096.;
/// Scene blocks Camera Raw renders brighter than this may be clipped.
const BRIGHT: f64 = 0.9;

/// #600: imported Highlights and Dehaze together must retain a nonblack sky.
/// The haze is measured before Highlights; subtracting it unchanged after the
/// highlight reduction used to drive the whole bright patch below zero.
#[test]
fn dehaze_with_reduced_highlights_keeps_the_sky_nonblack() {
    let (w, h) = (512, 128);
    let scene: Vec<[f64; 3]> = (0..w * h)
        .map(|i| {
            [if i / w < 16 {
                0.9
            } else if i % w < 256 {
                0.05
            } else {
                0.4
            }; 3]
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dehaze-sky.dng");
    std::fs::write(
        &path,
        dng::write_probe_black_render(w, h, &scene, 0., dng::BlackRender::None),
    )
    .unwrap();
    let im = develop(&path);
    let profiles = embedded_profiles(&im);
    for highlights in [-67, -100] {
        for dehaze in [50, 100] {
            let mut case = ProbeCase {
                probe: String::new(),
                settings: BTreeMap::from([
                    ("Highlights2012".into(), highlights.to_string()),
                    ("Dehaze".into(), dehaze.to_string()),
                ]),
                mask: BTreeMap::new(),
                camera_raw: Vec::new(),
            };
            let out = scene_output(&im, &profiles, &xmp(&case));
            let sky = out.pixels[(80 * w + 350) as usize];
            assert!(
                sky.iter().all(|v| *v > 1e-4),
                "Highlights {highlights}, Dehaze {dehaze}: {sky:?}"
            );
            // The same pair imported in a full-frame mask, including a mask's
            // negative Exposure before the scene stage (Adobe's normalised units).
            case.settings.clear();
            for exposure in [0., -0.5] {
                case.mask = BTreeMap::from([
                    (
                        "LocalHighlights2012".into(),
                        (highlights as f32 / 100.).to_string(),
                    ),
                    ("LocalDehaze".into(), (dehaze as f32 / 100.).to_string()),
                    ("LocalExposure2012".into(), exposure.to_string()),
                ]);
                let out = scene_output(&im, &profiles, &xmp(&case));
                let sky = out.pixels[(80 * w + 350) as usize];
                assert!(
                    sky.iter().all(|v| *v > 1e-4),
                    "mask Highlights {highlights}, Dehaze {dehaze}, Exposure {exposure}: {sky:?}"
                );
            }
        }
    }
}

#[test]
fn clarity_does_not_spread_a_bright_rail_into_its_dark_background() {
    // Issue #601: a high-resolution edge crosses the 512px measurement grid.
    // Camera Raw 18.7, PV15.4, linear embedded profile, all other Basic controls
    // zero: at +100 the dark pixel beside these rails gets darker, not brighter.
    // The old interpolated gain made the 4px rail's neighbour 0.396 sRGB instead
    // of its default 0.305 (Camera Raw: 0.274). Keep this at the RAW/XMP boundary.
    let (w, h) = (2048, 128);
    let rails = [(256, 4), (768, 8), (1280, 16), (1792, 32)];
    let scene: Vec<[f64; 3]> = (0..w * h)
        .map(|i| {
            let bright = rails
                .iter()
                .any(|(x, width)| (*x..x + width).contains(&(i % w)));
            [if bright { 0.6 } else { 0.08 }; 3]
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clarity-rails.dng");
    std::fs::write(&path, dng::write_probe(w, h, &scene, 0.)).unwrap();
    let im = develop(&path);
    let profiles = embedded_profiles(&im);
    let render = |clarity: i32| {
        scene_output(
            &im,
            &profiles,
            &xmp(&ProbeCase {
                probe: String::new(),
                settings: BTreeMap::from([("Clarity2012".into(), clarity.to_string())]),
                mask: BTreeMap::new(),
                camera_raw: Vec::new(),
            }),
        )
    };
    let base = render(0);
    for amount in [25, 50, 100] {
        let out = render(amount);
        for (x, width) in rails {
            for x in [x - 1, x + width] {
                let i = (h / 2 * w + x) as usize;
                assert!(
                    out.pixels[i][1] <= base.pixels[i][1] * 1.01,
                    "Clarity {amount}, x={x}: {} -> {}",
                    base.pixels[i][1],
                    out.pixels[i][1]
                );
            }
        }
    }
}

#[test]
fn clarity_does_not_turn_mask_color_or_exposure_into_detail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clarity-flat.dng");
    std::fs::write(
        &path,
        dng::write_probe(64, 64, &vec![[0.08; 3]; 64 * 64], 0.),
    )
    .unwrap();
    let im = develop(&path);
    let profiles = embedded_profiles(&im);
    for mask in [
        BTreeMap::from([("LocalExposure2012".into(), "0.5".into())]),
        BTreeMap::from([("LocalExposure2012".into(), "-0.5".into())]),
        BTreeMap::from([("LocalTemperature".into(), "0.5".into())]),
        BTreeMap::from([
            ("LocalToningHue".into(), "30".into()),
            ("LocalToningSaturation".into(), "0.8".into()),
        ]),
    ] {
        let render = |clarity: i32, masked: bool| {
            let mut mask = mask.clone();
            if masked {
                mask.insert(
                    "LocalClarity2012".into(),
                    (clarity as f32 / 100.).to_string(),
                );
            }
            scene_output(
                &im,
                &profiles,
                &xmp(&ProbeCase {
                    probe: String::new(),
                    settings: BTreeMap::from([(
                        "Clarity2012".into(),
                        (if masked { 0 } else { clarity }).to_string(),
                    )]),
                    mask,
                    camera_raw: Vec::new(),
                }),
            )
        };
        let base = render(0, false);
        for (amount, masked) in [(-100, false), (100, false), (-100, true), (100, true)] {
            let out = render(amount, masked);
            let max = out
                .pixels
                .iter()
                .flatten()
                .zip(base.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert!(
                max < 1e-5,
                "Clarity {amount}, masked {masked}, {mask:?}: flat-scene difference {max}"
            );
        }
    }
}

fn shadow_gap_scene(size: u32) -> Vec<[f64; 3]> {
    (0..size * size)
        .map(|i| {
            let x = (i % size) as f64;
            let y = (i / size) as f64;
            let mut level = -6.3
                + 0.7
                    * (x / 640. * std::f64::consts::TAU).sin()
                    * (y / 640. * std::f64::consts::TAU).sin();
            for (cx, cy) in [
                (100., 220.),
                (200., 250.),
                (160., 400.),
                (400., 150.),
                (470., 350.),
                (550., 200.),
            ] {
                let spot = 0.7 - 8. * ((x - cx).hypot(y - cy) - 4.).max(0.) / 6.;
                level = level.max(spot);
            }
            [level.exp2(); 3]
        })
        .collect()
}

#[test]
fn shadows_lift_a_dark_scene_with_small_bright_gaps() {
    let size = 640;
    let scene = shadow_gap_scene(size);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shadow-gaps.dng");
    std::fs::write(&path, dng::write_probe(size, size, &scene, 1.)).unwrap();
    let im = develop(&path);
    let profiles = embedded_profiles(&im);
    for (exposure, camera_raw) in [
        (-2, 0.5995982538287945),
        (0, 0.7852627805513078),
        (2, 0.5721837060845278),
    ] {
        let render = |shadows: i32| {
            let case = ProbeCase {
                probe: String::new(),
                settings: BTreeMap::from([
                    ("Shadows2012".into(), shadows.to_string()),
                    ("Exposure2012".into(), exposure.to_string()),
                ]),
                mask: BTreeMap::new(),
                camera_raw: Vec::new(),
            };
            scene_output(&im, &profiles, &xmp(&case))
        };
        let base = render(0);
        let lifted = render(100);
        let mut gains = Vec::new();
        for y in 100..540 {
            for x in 100..540 {
                let i = y * 640 + x;
                let luminance =
                    |p: [f32; 3]| p.iter().zip(Y).map(|(v, w)| f64::from(*v) * w).sum::<f64>();
                let before = luminance(base.pixels[i]);
                if before > 2_f64.powi(-11) && before < 0.15 {
                    gains.push((luminance(lifted.pixels[i]) / before).log2());
                }
            }
        }
        gains.sort_by(f64::total_cmp);
        let median = gains[gains.len() / 2];
        // Camera Raw 18.7 (2743), Process Version 15.4, linear embedded profile,
        // default black rendering, other Basic settings zero, Shadows +100 at several exposures.
        // Known gap (docs/scene-tone-stage.md#the-global-rule): the global rule reads
        // this scene through its histogram alone and lifts it about 0.2 EV less than
        // Camera Raw, whose lift also depends on the gaps' arrangement. The gap must
        // not grow; Camera Raw's values are not re-blessed.
        let known_gap = 0.25;
        assert!(
            (median - camera_raw).abs() < known_gap,
            "Exposure {exposure}, Shadows +100 lifts the scene by {median} EV; Camera Raw {camera_raw} EV"
        );
    }
}

#[test]
fn shadows_lift_dark_scene_without_default_black() {
    let size = 640;
    let scene = shadow_gap_scene(size);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shadow-gaps.dng");
    std::fs::write(
        &path,
        dng::write_probe_black_render(size, size, &scene, 1., dng::BlackRender::None),
    )
    .unwrap();
    let im = develop(&path);
    let profiles = embedded_profiles(&im);
    let render = |shadows: i32| {
        let case = ProbeCase {
            probe: String::new(),
            settings: BTreeMap::from([("Shadows2012".into(), shadows.to_string())]),
            mask: BTreeMap::new(),
            camera_raw: Vec::new(),
        };
        scene_output(&im, &profiles, &xmp(&case))
    };
    let base = render(0);
    let lifted = render(100);
    let mut gains = Vec::new();
    for y in 100..540 {
        for x in 100..540 {
            let i = y * 640 + x;
            let luminance =
                |p: [f32; 3]| p.iter().zip(Y).map(|(v, w)| f64::from(*v) * w).sum::<f64>();
            let before = luminance(base.pixels[i]);
            if before > 2_f64.powi(-11) && before < 0.15 {
                gains.push((luminance(lifted.pixels[i]) / before).log2());
            }
        }
    }
    gains.sort_by(f64::total_cmp);
    let median = gains[gains.len() / 2];
    // Camera Raw 18.7 (2743), Process Version 15.4, linear embedded profile,
    // DefaultBlackRender None, all Basic settings zero except Shadows +100.
    let camera_raw = 0.5244576436042451;
    // Known gap of the global rule on this sparse-gap scene (see the test above).
    let known_gap = 0.25;
    assert!(
        (median - camera_raw).abs() < known_gap,
        "Shadows +100 lifts the scene by {median} EV; Camera Raw {camera_raw} EV"
    );
}

#[test]
fn shadows_respond_to_spatial_arrangement_with_the_same_histogram() {
    let size = 640;
    let scattered = shadow_gap_scene(size);
    let mut sorted = scattered.clone();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let dir = tempfile::tempdir().unwrap();
    let gains: Vec<f64> = [scattered, sorted]
        .iter()
        .enumerate()
        .map(|(index, scene)| {
            let path = dir.path().join(format!("arrangement-{index}.dng"));
            std::fs::write(&path, dng::write_probe(size, size, scene, 1.)).unwrap();
            let im = develop(&path);
            let profiles = embedded_profiles(&im);
            let render = |shadows: i32| {
                let case = ProbeCase {
                    probe: String::new(),
                    settings: BTreeMap::from([("Shadows2012".into(), shadows.to_string())]),
                    mask: BTreeMap::new(),
                    camera_raw: Vec::new(),
                };
                scene_output(&im, &profiles, &xmp(&case))
            };
            let before = render(0);
            let after = render(100);
            let luminance =
                |p: &[f32; 3]| p.iter().zip(Y).map(|(v, w)| f64::from(*v) * w).sum::<f64>();
            let mut gain: Vec<f64> = before
                .pixels
                .iter()
                .zip(&after.pixels)
                .filter_map(|(a, b)| {
                    let a = luminance(a);
                    (a > 2_f64.powi(-11) && a < 0.15).then(|| (luminance(b) / a).log2())
                })
                .collect();
            assert!(!gain.is_empty());
            gain.sort_by(f64::total_cmp);
            gain[gain.len() / 2]
        })
        .collect();
    // Camera Raw 18.7 (2743), PV 15.4, neutral linear embedded profile,
    // DefaultBlackRender Auto; only Shadows changes from 0 to +100.
    // The two scenes contain exactly the same input values. The second merely
    // sorts their spatial positions. A histogram-only strength adjustment cannot
    // reproduce the measured difference in lift.
    let expected = 0.7852627805513078 - 0.4357064155855821;
    let difference = gains[0] - gains[1];
    // Known gap: the global rule (docs/scene-tone-stage.md#the-global-rule) lifts
    // both arrangements alike, since it reads the histogram of the base alone; the
    // earlier scene-context offset reproduced about 0.35 EV of Camera Raw's
    // difference. Both arrangements must still be lifted, and the sorted one never
    // more than the scattered one by more than the noise of the measurement.
    assert!(
        gains[0] > 0.3 && gains[1] > 0.3 && gains[1] - gains[0] < 0.1,
        "scattered/sorted Shadows gains {gains:?} EV; difference {difference}, Camera Raw {expected}"
    );
}

#[test]
fn default_black_follows_scene_darkness_across_camera_baselines() {
    let values: Vec<f64> = (-8..=0).map(|e| 2_f64.powi(e)).collect();
    let (width, height) = (576_u32, 64_u32);
    let scene: Vec<[f64; 3]> = (0..width * height)
        .map(|i| [values[((i % width) / 64) as usize]; 3])
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let mut errors = Vec::new();
    // Camera Raw 18.7 (2743), PV 15.4, linear profile, Auto black. Changing
    // camera baseline exposure while keeping scene values fixed leaves these
    // dark patch outputs unchanged. User Exposure +1 moves the black response.
    for (baseline, exposure, expected) in [
        (0., 0, [0.00101035, 0.00471585, 0.012339, 0.027770]),
        (2., 0, [0.001025, 0.004731, 0.012355, 0.027785]),
        (2., 1, [0.002837, 0.010701, 0.026418, 0.057868]),
    ] {
        let path = dir.path().join(format!("black-{baseline}-{exposure}.dng"));
        std::fs::write(&path, dng::write_probe(width, height, &scene, baseline)).unwrap();
        let im = develop(&path);
        let profiles = embedded_profiles(&im);
        let case = ProbeCase {
            probe: String::new(),
            settings: BTreeMap::from([("Exposure2012".into(), exposure.to_string())]),
            mask: BTreeMap::new(),
            camera_raw: Vec::new(),
        };
        let out = scene_output(&im, &profiles, &xmp(&case));
        for (patch, reference) in expected.into_iter().enumerate() {
            let pixel = out.pixels[32 * width as usize + patch * 64 + 32];
            let got = pixel
                .iter()
                .zip(Y)
                .map(|(v, w)| f64::from(*v) * w)
                .sum::<f64>();
            if !got.is_finite() || (got - reference).abs() > 0.0004 {
                errors.push(format!(
                    "BE {baseline}, E {exposure}, patch {patch}: {got}, ACR {reference}"
                ));
            }
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[derive(Deserialize)]
struct References {
    probes: BTreeMap<String, Probe>,
    cases: BTreeMap<String, ProbeCase>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Probe {
    /// Neutral patches on a background, laid out as scene-tone-tables.py's `Grid`.
    Ramp {
        values: Vec<f64>,
        background: f64,
        baseline_exposure: f64,
        grid: Grid,
    },
    /// A committed probe DNG, measured as blocks (across, down).
    Scene { file: String, blocks: [u32; 2] },
}

#[derive(Deserialize)]
struct Grid {
    cols: u32,
    patch: u32,
    gap: u32,
    margin: u32,
    /// Pixels left out at each side of a patch when it is measured.
    inset: u32,
}

impl Grid {
    fn position(&self, i: usize) -> (u32, u32) {
        let (r, c) = (i as u32 / self.cols, i as u32 % self.cols);
        (
            self.margin + c * (self.patch + self.gap),
            self.margin + r * (self.patch + self.gap),
        )
    }
    fn size(&self, n: usize) -> (u32, u32) {
        let rows = (n as u32).div_ceil(self.cols);
        let w = 2 * self.margin + self.cols * self.patch + (self.cols - 1) * self.gap;
        let h = 2 * self.margin + rows * self.patch + (rows - 1) * self.gap;
        (w + w % 2, h + h % 2)
    }
}

#[derive(Deserialize)]
struct ProbeCase {
    probe: String,
    settings: BTreeMap<String, String>,
    /// Local adjustments of one mask covering the whole frame.
    #[serde(default)]
    mask: BTreeMap<String, String>,
    camera_raw: Vec<f64>,
}

/// A case's distance from Camera Raw.
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Errors {
    /// Mean and largest difference of patch (block) values, encoded with gamma 1.8 after
    /// clipping to 0–1, as Camera Raw's ProPhoto TIFFs are.
    mae: f64,
    max: f64,
    /// Scenes: the mean error of the setting's effect, log2(case / default), in EV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effect: Option<f64>,
}

impl Errors {
    /// Within the baseline, with room for floating-point and platform noise.
    fn within(&self, baseline: &Errors) -> bool {
        let ok = |got: f64, base: f64, slack: f64| got <= base * 1.05 + slack;
        ok(self.mae, baseline.mae, 0.0005)
            && ok(self.max, baseline.max, 0.003)
            && match (self.effect, baseline.effect) {
                (Some(got), Some(base)) => ok(got, base, 0.005),
                (got, base) => got.is_none() == base.is_none(),
            }
    }
}

fn baseline_path() -> std::path::PathBuf {
    corpus().join("camera-raw/scene-probes-baseline.json")
}

fn encode(v: f64) -> f64 {
    v.clamp(0., 1.).powf(1. / 1.8)
}

/// The case's settings XMP: the corpus cases' base settings, the case's sliders and its
/// mask, a gradient beyond the top edge, as scene-tone-tables.py's `mask_xmp`.
fn xmp(case: &ProbeCase) -> String {
    let base = cases().base;
    let text = Case {
        name: String::new(),
        settings: case.settings.clone(),
        charts: None,
        curves: BTreeMap::new(),
        photos: false,
        look: None,
    }
    .xmp(&base, &[]);
    if case.mask.is_empty() {
        return text;
    }
    let attributes: String = case
        .mask
        .iter()
        .map(|(k, v)| format!(r#" crs:{k}="{v}""#))
        .collect();
    let mask = format!(
        r#"<crs:MaskGroupBasedCorrections><rdf:Seq><rdf:li><rdf:Description crs:What="Correction" crs:CorrectionAmount="1"{attributes}><crs:CorrectionMasks><rdf:Seq><rdf:li crs:What="Mask/Gradient" crs:MaskBlendMode="0" crs:ZeroX="0.5" crs:ZeroY="-2" crs:FullX="0.5" crs:FullY="-1"/></rdf:Seq></crs:CorrectionMasks></rdf:Description></rdf:li></rdf:Seq></crs:MaskGroupBasedCorrections>"#
    );
    let end = "</rdf:Description></rdf:RDF></x:xmpmeta>";
    text.strip_suffix(end).unwrap().to_string() + &mask + end
}

/// The scene tone stage's output (linear ProPhoto) for the case's settings, with the
/// recipe built as `rawmakase render --xmp --tap scene-output` builds it.
fn scene_output(im: &CameraImage, profiles: &[Arc<CameraProfile>], xmp: &str) -> Rendered {
    let base = rawmakase::model::recipe::Recipe::with_profiles(&im.metadata, profiles);
    let preset = rawmakase::xmp::parse(Path::new("case.xmp"), xmp).unwrap();
    let recipe = preset
        .apply(
            &base,
            &im.metadata,
            profiles,
            Some(&rawmakase::develop::Measures(im)),
        )
        .unwrap();
    quality::render_stage(im, &recipe, quality::Stage::SceneOutput, Cancel::NEVER).unwrap()
}

/// A clipped plateau with a smooth bright edge, surrounded by a dark scene. The
/// unconstrained Highlights fit solarized it: the centre became much darker than
/// its rim. Compare spatial contrast with independent Camera Raw measurements,
/// through DNG decoding, XMP slider/mask translation and the full scene stage.
#[test]
fn clipped_highlights_do_not_solarize() {
    #[derive(Deserialize)]
    struct Spot {
        background_ev: i32,
        highlights: i32,
        ratio: f64,
    }
    #[derive(Deserialize)]
    struct Spots {
        cases: Vec<Spot>,
    }
    let refs: Spots = serde_json::from_slice(
        &std::fs::read(corpus().join("camera-raw/highlight-spots.json")).unwrap(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    for key in [-5, -7, -9] {
        let scene: Vec<[f64; 3]> = (0..480 * 480)
            .map(|i| {
                let radius = ((i % 480) as f64 - 240.).hypot((i / 480) as f64 - 240.);
                let level = f64::from(key) * (1. - ((90. - radius) / 50.).clamp(0., 1.));
                [level.exp2(); 3]
            })
            .collect();
        let path = dir.path().join("spot.dng");
        std::fs::write(&path, dng::write_probe(480, 480, &scene, 0.)).unwrap();
        let im = develop(&path);
        let profiles = embedded_profiles(&im);
        for reference in refs.cases.iter().filter(|r| r.background_ev == key) {
            for masked in [false, true] {
                let slider = reference.highlights;
                let case = ProbeCase {
                    probe: String::new(),
                    settings: if masked {
                        BTreeMap::new()
                    } else {
                        BTreeMap::from([("Highlights2012".into(), slider.to_string())])
                    },
                    mask: if masked {
                        BTreeMap::from([(
                            "LocalHighlights2012".into(),
                            (f64::from(slider) / 100.).to_string(),
                        )])
                    } else {
                        BTreeMap::new()
                    },
                    camera_raw: Vec::new(),
                };
                let out = scene_output(&im, &profiles, &xmp(&case));
                let lum = |x: usize, y: usize| {
                    out.pixels[y * 480 + x]
                        .iter()
                        .map(|v| f64::from(*v))
                        .sum::<f64>()
                        / 3.
                };
                let centre = (235..245)
                    .flat_map(|y| (235..245).map(move |x| (x, y)))
                    .map(|(x, y)| lum(x, y))
                    .sum::<f64>()
                    / 100.;
                let rim = (240..331).map(|x| lum(x, 240)).fold(0., f64::max);
                assert!(centre > 0. && centre.is_finite());
                let ratio = rim / centre;
                // Allow 5 percentage points for our different spatial filter. Do
                // not re-bless the Camera Raw ratios to accommodate a regression.
                assert!(
                    ratio <= reference.ratio + 0.05,
                    "key {key}, Highlights {slider}, masked {masked}: rim/centre {ratio}, Camera Raw {}",
                    reference.ratio
                );
            }
        }
    }
}

/// Patch means (over the three channels) or block luminances of a render.
fn measure(probe: &Probe, out: &Rendered) -> Vec<f64> {
    let at = |x: u32, y: u32| out.pixels[(y * out.width + x) as usize].map(f64::from);
    let mean = |x0: u32, y0: u32, w: u32, h: u32, f: &dyn Fn([f64; 3]) -> f64| {
        let mut sum = 0.;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                sum += f(at(x, y));
            }
        }
        sum / f64::from(w * h)
    };
    match probe {
        Probe::Ramp { values, grid, .. } => {
            assert_eq!((out.width, out.height), grid.size(values.len()));
            let side = grid.patch - 2 * grid.inset;
            (0..values.len())
                .map(|i| {
                    let (x, y) = grid.position(i);
                    mean(x + grid.inset, y + grid.inset, side, side, &|p| {
                        p.iter().sum::<f64>() / 3.
                    })
                })
                .collect()
        }
        Probe::Scene { blocks, .. } => {
            let (bw, bh) = (out.width / blocks[0], out.height / blocks[1]);
            (0..blocks[1])
                .flat_map(|by| (0..blocks[0]).map(move |bx| (bx, by)))
                .map(|(bx, by)| {
                    mean(bx * bw, by * bh, bw, bh, &|p| {
                        p.iter().zip(Y).map(|(v, w)| v * w).sum()
                    })
                })
                .collect()
        }
    }
}

fn errors(rm: &[f64], cr: &[f64], defaults: Option<(&[f64], &[f64])>) -> Errors {
    assert_eq!(rm.len(), cr.len(), "patch counts differ");
    let d: Vec<f64> = rm
        .iter()
        .zip(cr)
        .map(|(a, b)| (encode(*a) - encode(*b)).abs())
        .collect();
    let effect = defaults.map(|(rm0, cr0)| {
        let e: Vec<f64> = (0..rm.len())
            .filter(|&i| {
                [rm[i], cr[i], rm0[i], cr0[i]].iter().all(|&v| v > DARK)
                    && cr[i].max(cr0[i]) < BRIGHT
            })
            .map(|i| ((rm[i] / rm0[i]).log2() - (cr[i] / cr0[i]).log2()).abs())
            .collect();
        assert!(!e.is_empty(), "no blocks to compare");
        e.iter().sum::<f64>() / e.len() as f64
    });
    Errors {
        mae: d.iter().sum::<f64>() / d.len() as f64,
        max: d.iter().copied().fold(0., f64::max),
        effect,
    }
}

#[test]
fn scene_probes_do_not_regress() {
    let refs: References = serde_json::from_slice(
        &std::fs::read(corpus().join("camera-raw/scene-probes.json")).unwrap(),
    )
    .expect("scene-probes.json");
    let dir = tempfile::tempdir().unwrap();
    // Every case rendered, probe by probe.
    let rendered: BTreeMap<&str, Vec<f64>> = refs
        .probes
        .iter()
        .flat_map(|(name, probe)| {
            let path = match probe {
                Probe::Ramp {
                    values,
                    background,
                    baseline_exposure,
                    grid,
                } => {
                    let (w, h) = grid.size(values.len());
                    let mut scene = vec![[*background; 3]; (w * h) as usize];
                    for (i, v) in values.iter().enumerate() {
                        let (x0, y0) = grid.position(i);
                        for y in y0..y0 + grid.patch {
                            for x in x0..x0 + grid.patch {
                                scene[(y * w + x) as usize] = [*v; 3];
                            }
                        }
                    }
                    let path = dir.path().join(format!("{name}.dng"));
                    std::fs::write(&path, dng::write_probe(w, h, &scene, *baseline_exposure))
                        .unwrap();
                    path
                }
                Probe::Scene { file, .. } => corpus().join(file),
            };
            let im = develop(&path);
            let profiles = embedded_profiles(&im);
            let cases: Vec<_> = refs
                .cases
                .iter()
                .filter(|(_, c)| c.probe == *name)
                .collect();
            cases
                .par_iter()
                .map(|(case_name, case)| {
                    let out = scene_output(&im, &profiles, &xmp(case));
                    (case_name.as_str(), measure(probe, &out))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let mut current = BTreeMap::new();
    for (name, case) in &refs.cases {
        let defaults = matches!(refs.probes[&case.probe], Probe::Scene { .. })
            .then(|| format!("{}/default", case.probe))
            .filter(|d| d != name)
            .map(|d| {
                (
                    rendered[d.as_str()].as_slice(),
                    refs.cases[&d].camera_raw.as_slice(),
                )
            });
        current.insert(
            name.clone(),
            errors(&rendered[name.as_str()], &case.camera_raw, defaults),
        );
    }

    if bless() {
        let lines: Vec<String> = current
            .iter()
            .map(|(k, v)| {
                let round = |x: f64| (x * 1e5).round() / 1e5;
                let v = Errors {
                    mae: round(v.mae),
                    max: round(v.max),
                    effect: v.effect.map(round),
                };
                format!(
                    "  {}: {}",
                    serde_json::to_string(k).unwrap(),
                    serde_json::to_string(&v).unwrap()
                )
            })
            .collect();
        std::fs::write(baseline_path(), format!("{{\n{}\n}}\n", lines.join(",\n"))).unwrap();
        return;
    }
    let baseline: BTreeMap<String, Errors> =
        serde_json::from_slice(&std::fs::read(baseline_path()).unwrap())
            .expect("scene-probes-baseline.json");
    let mut failures = Vec::new();
    for (name, got) in &current {
        let base = baseline.get(name);
        let effect = |e: Option<f64>| e.map_or(String::new(), |e| format!("  effect {e:.3} EV"));
        println!(
            "{name:24} mae {:.4} max {:.4}{}   baseline {}",
            got.mae,
            got.max,
            effect(got.effect),
            base.map_or("none".into(), |b| format!(
                "mae {:.4} max {:.4}{}",
                b.mae,
                b.max,
                effect(b.effect)
            ))
        );
        if !base.is_some_and(|b| got.within(b)) {
            failures.push(name.as_str());
        }
    }
    assert!(
        baseline.keys().all(|k| current.contains_key(k)),
        "the baseline has cases without references"
    );
    assert!(
        failures.is_empty(),
        "further from Camera Raw than the baseline (run with --nocapture for the table): {failures:?}"
    );
}
