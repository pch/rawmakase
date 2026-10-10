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
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

/// ProPhoto luminance.
const Y: [f64; 3] = [0.2880402, 0.7118741, 0.0000857];
/// Scene blocks darker than this (either render) are left out of the effect error.
const DARK: f64 = 1. / 4096.;
/// Scene blocks Camera Raw renders brighter than this may be clipped.
const BRIGHT: f64 = 0.9;

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
    quality::render_stage(
        im,
        &recipe,
        quality::Stage::SceneOutput,
        &AtomicBool::new(false),
    )
    .unwrap()
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
