//! Color tests on synthetic chart DNGs; see tests/corpus/README.md.
//!
//! Public tests (run by `cargo test`): the committed charts match their generator
//! and decode exactly; renders match RAWmakase's own committed snapshots (catches
//! any color change); exports and preview-size renders match full renders; and
//! renders are no further from Camera Raw than the recorded baseline.
//!
//! Private tests (`--ignored`, need RAWMAKASE_CORPUS and sometimes
//! RAWMAKASE_PROFILES): Adobe-profile parity on per-camera charts, and real photos
//! against their last accepted renders.
//!
//! RAWMAKASE_BLESS=1 rewrites charts, snapshots and baselines instead of comparing.
mod black_render;
mod chart;
mod dng;
mod fit_chart;
mod measure;
mod private;
mod red_eye;
mod retouch;
mod scene_probes;

use chart::{Camera, Illuminant, Layout, Patch};
use measure::{chroma, delta_e2000, hue_difference, lab};
use rawmakase::{camera_data::CameraImage, camera_profiles::CameraProfile, rendered::Rendered};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

pub fn bless() -> bool {
    std::env::var_os("RAWMAKASE_BLESS").is_some_and(|v| v != "0")
}

pub struct ChartSpec {
    pub name: String,
    pub camera: Camera,
    pub illuminant: Illuminant,
    /// Embed a named profile with forward matrices, as Adobe's DNG Converter does.
    /// Without it the DNG carries color matrices only.
    pub profile: bool,
}

/// Every committed chart: the synthetic camera under four illuminants with an
/// embedded profile; once with color matrices only, as some third-party DNGs are;
/// and one chart per camera in cameras.json.
pub fn chart_specs() -> Vec<ChartSpec> {
    let mut specs: Vec<ChartSpec> = [
        Illuminant::D65,
        Illuminant::D50,
        Illuminant::A,
        Illuminant::F2,
    ]
    .into_iter()
    .map(|illuminant| ChartSpec {
        name: format!("synthetic-{}", illuminant.name()),
        camera: Camera::synthetic(),
        illuminant,
        profile: true,
    })
    .collect();
    specs.push(ChartSpec {
        name: "synthetic-d65-matrix-only".into(),
        camera: Camera::synthetic(),
        illuminant: Illuminant::D65,
        profile: false,
    });
    for camera in cameras() {
        specs.push(ChartSpec {
            name: format!("{}-d65", camera.id),
            camera: camera.normalised(),
            illuminant: Illuminant::D65,
            profile: true,
        });
    }
    specs
}

pub fn cameras() -> Vec<Camera> {
    let path = corpus().join("cameras.json");
    if !path.exists() {
        return Vec::new();
    }
    serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("cameras.json")
}

pub fn chart_path(name: &str) -> PathBuf {
    corpus().join("charts").join(format!("{name}.dng"))
}

pub fn generate(spec: &ChartSpec, layout: &Layout) -> Vec<u8> {
    let rendered = chart::render(layout, &spec.camera, spec.illuminant);
    dng::write(
        &dng::Image {
            width: chart::WIDTH,
            height: chart::HEIGHT,
            camera: &rendered.camera,
            as_shot_neutral: rendered.as_shot_neutral,
            profile: spec.profile,
            black_render: dng::BlackRender::Auto,
        },
        &spec.camera,
    )
}

#[derive(Deserialize)]
pub struct Cases {
    pub base: BTreeMap<String, String>,
    pub cases: Vec<Case>,
}

#[derive(Clone, Deserialize)]
pub struct Case {
    pub name: String,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    #[serde(default)]
    pub charts: Option<Vec<String>>,
    #[serde(default)]
    pub curves: BTreeMap<String, Vec<String>>,
    /// Also rendered on the private real photos.
    #[serde(default)]
    pub photos: bool,
    /// A look profile from `looks/`, applied at a Profile Amount.
    #[serde(default)]
    pub look: Option<CaseLook>,
}

#[derive(Clone, Deserialize)]
pub struct CaseLook {
    /// File name in tests/corpus/looks.
    pub file: String,
    /// Lightroom's `Amount`: 1 is 100%.
    pub amount: f32,
}

impl CaseLook {
    pub fn path(&self) -> PathBuf {
        corpus().join("looks").join(&self.file)
    }
    /// The look's name and UUID, as a sidecar's `crs:Look` names it.
    fn identity(&self) -> (String, String) {
        let text = std::fs::read_to_string(self.path()).unwrap();
        let attribute = |key: &str| {
            let start = text
                .find(key)
                .unwrap_or_else(|| panic!("{}: {key}", self.file))
                + key.len();
            text[start..start + text[start..].find(['"', '<']).unwrap()].to_string()
        };
        (
            attribute(r#"<rdf:li xml:lang="x-default">"#),
            attribute(r#"crs:UUID=""#),
        )
    }
}

impl Case {
    /// Cases run on synthetic-d65 unless they list other charts ("*" = all).
    pub fn applies(&self, chart: &str) -> bool {
        match &self.charts {
            None => chart == "synthetic-d65",
            Some(list) => list.iter().any(|c| c == "*" || c == chart),
        }
    }

    /// The case as a Camera Raw settings XMP, plus any `extra` attributes.
    pub fn xmp(&self, base: &BTreeMap<String, String>, extra: &[(&str, &str)]) -> String {
        let mut attributes = base.clone();
        attributes.extend(self.settings.clone());
        for (k, v) in extra {
            attributes.insert(k.to_string(), v.to_string());
        }
        let mut xml = String::from(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:HasSettings="True""#,
        );
        for (k, v) in attributes {
            xml += &format!(r#" crs:{k}="{v}""#);
        }
        xml += ">";
        if let Some(look) = &self.look {
            let (name, uuid) = look.identity();
            xml += &format!(
                r#"<crs:Look><rdf:Description crs:Name="{name}" crs:Amount="{}" crs:UUID="{uuid}"/></crs:Look>"#,
                look.amount
            );
        }
        for (name, points) in &self.curves {
            xml += &format!("<crs:{name}><rdf:Seq>");
            for p in points {
                xml += &format!("<rdf:li>{p}</rdf:li>");
            }
            xml += &format!("</rdf:Seq></crs:{name}>");
        }
        xml + "</rdf:Description></rdf:RDF></x:xmpmeta>"
    }
}

pub fn cases() -> Cases {
    serde_json::from_slice(&std::fs::read(corpus().join("cases.json")).unwrap())
        .expect("cases.json")
}

pub fn develop(path: &Path) -> CameraImage {
    charts_ready();
    rawmakase::photo::open(path)
        .and_then(|r| {
            r.develop(
                rawmakase::camera_data::Decode::full(Default::default()),
                &AtomicBool::new(false),
            )
        })
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Renders a case the way `rawmakase render --xmp` does: the default recipe for the
/// file and the given profiles, with the case's settings applied on top.
pub fn render(
    im: &CameraImage,
    profiles: &[Arc<CameraProfile>],
    xmp: &str,
    max_edge: u32,
) -> anyhow::Result<Rendered> {
    let base = rawmakase::model::recipe::Recipe::with_profiles(&im.metadata, profiles);
    let preset = rawmakase::xmp::parse(Path::new("case.xmp"), xmp)?;
    let recipe = preset.apply(
        &base,
        &im.metadata,
        profiles,
        Some(&rawmakase::develop::Measures(im)),
    )?;
    rawmakase::develop::render(im, &recipe.checked()?, max_edge)
}

/// The DNG's own embedded profile, and nothing from the user's library. A DNG
/// written with colour matrices alone has no embedded profile, and then the app
/// falls back to the camera matrix with the DNG tone curve, so the test does too.
pub fn embedded_profiles(im: &CameraImage) -> Vec<Arc<CameraProfile>> {
    rawmakase::camera_profiles::builtin(&im.metadata)
        .or_else(|| {
            rawmakase::camera_profiles::open::standard(&im.metadata).map(std::sync::Arc::new)
        })
        .into_iter()
        .collect()
}

pub type PatchValues = Vec<[u16; 3]>;

/// A JSON file of patch values per case, one case per line for readable diffs.
#[derive(Default, Serialize, Deserialize)]
pub struct PatchFile {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub about: BTreeMap<String, String>,
    pub cases: BTreeMap<String, PatchValues>,
}

impl PatchFile {
    pub fn read(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        Some(serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
    }
    pub fn write(&self, path: &Path) {
        let mut out = String::from("{\n");
        if !self.about.is_empty() {
            out += &format!(
                " \"about\": {},\n",
                serde_json::to_string(&self.about).unwrap()
            );
        }
        out += " \"cases\": {\n";
        let lines: Vec<String> = self
            .cases
            .iter()
            .map(|(k, v)| {
                format!(
                    "  {}: {}",
                    serde_json::to_string(k).unwrap(),
                    serde_json::to_string(v).unwrap()
                )
            })
            .collect();
        out += &lines.join(",\n");
        out += "\n }\n}\n";
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, out).unwrap();
    }
}

/// Differences between two sets of patch values.
pub struct Comparison {
    pub delta_e: Vec<f64>,
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
}

/// Panics when the two sets are not the same nonempty size: a truncated or empty
/// reference would otherwise compare as a perfect match.
pub fn compare(
    reference: &[[u16; 3]],
    candidate: &[[u16; 3]],
    keep: impl Fn(usize) -> bool,
) -> Comparison {
    assert!(!reference.is_empty(), "nothing to compare");
    assert_eq!(
        reference.len(),
        candidate.len(),
        "reference and candidate have different patch counts"
    );
    assert!(
        (0..reference.len()).any(&keep),
        "every patch is excluded from the comparison"
    );
    let delta_e: Vec<f64> = reference
        .iter()
        .zip(candidate)
        .map(|(a, b)| delta_e2000(lab(*a), lab(*b)))
        .collect();
    let mut kept: Vec<f64> = delta_e
        .iter()
        .enumerate()
        .filter(|(i, _)| keep(*i))
        .map(|(_, d)| *d)
        .collect();
    kept.sort_by(f64::total_cmp);
    let mean = kept.iter().sum::<f64>() / kept.len().max(1) as f64;
    let p95 = kept
        .get(((kept.len() as f64 * 0.95).ceil() as usize).saturating_sub(1))
        .copied()
        .unwrap_or(0.);
    let max = kept.last().copied().unwrap_or(0.);
    Comparison {
        delta_e,
        mean,
        p95,
        max,
    }
}

/// Describes a patch difference as lightness, chroma and hue changes.
pub fn describe(patch: &Patch, reference: [u16; 3], candidate: [u16; 3], delta_e: f64) -> String {
    let (a, b) = (lab(reference), lab(candidate));
    format!(
        "{:28} ΔE00 {delta_e:5.2}  ΔL* {:+6.2}  ΔC* {:+6.2}  Δh {:+6.1}°",
        patch.name,
        b[0] - a[0],
        chroma(b) - chroma(a),
        if chroma(a).min(chroma(b)) > 2. {
            hue_difference(a, b)
        } else {
            0.
        }
    )
}

/// Renders every applicable case of a chart (or only the cases in `only`) and
/// measures its patches. A case that cannot be rendered (e.g. a missing profile)
/// is an error message instead.
pub fn render_chart(
    chart: &str,
    layout: &Layout,
    cases: &Cases,
    extra: &[(&str, &str)],
    profiles: impl Fn(&CameraImage) -> Vec<Arc<CameraProfile>>,
    only: Option<&BTreeMap<String, PatchValues>>,
) -> BTreeMap<String, Result<PatchValues, String>> {
    let im = develop(&chart_path(chart));
    let profiles = profiles(&im);
    cases
        .cases
        .par_iter()
        .filter(|c| match only {
            Some(names) => names.contains_key(&c.name),
            None => c.applies(chart),
        })
        .map(|c| {
            let mut profiles = profiles.clone();
            if let Some(look) = &c.look {
                // Composed over the chart's own profile, as the app does without
                // Adobe Standard.
                match rawmakase::camera_profiles::compose_look(
                    &look.path(),
                    &profiles,
                    rawmakase::camera_profiles::builtin(&im.metadata).as_deref(),
                ) {
                    Ok(p) => profiles.push(Arc::new(p)),
                    Err(e) => return (c.name.clone(), Err(format!("{}: {e:#}", look.file))),
                }
            }
            let result = render(&im, &profiles, &c.xmp(&cases.base, extra), 0)
                .map_err(|e| e.to_string())
                .and_then(|out| {
                    if (out.width, out.height) != (chart::WIDTH, chart::HEIGHT) {
                        return Err(format!("rendered at {}×{}", out.width, out.height));
                    }
                    Ok(measure::patches(out.width, &out.pixels, &layout.patches))
                });
            (c.name.clone(), result)
        })
        .collect()
}

fn layout_json(layout: &Layout) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "about": "Patch areas of every chart in this folder: x, y, w, h is the averaged area in pixels.",
        "width": chart::WIDTH,
        "height": chart::HEIGHT,
        "patches": layout.patches,
    }))
    .unwrap()
        + "\n"
}

/// With RAWMAKASE_BLESS, rewrites the charts once, before any test reads them.
fn charts_ready() {
    static READY: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    READY.get_or_init(|| {
        if bless() {
            let layout = Layout::new();
            std::fs::create_dir_all(corpus().join("charts")).unwrap();
            std::fs::write(corpus().join("charts/layout.json"), layout_json(&layout)).unwrap();
            for spec in chart_specs() {
                std::fs::write(chart_path(&spec.name), generate(&spec, &layout)).unwrap();
            }
        }
    });
}

#[test]
fn charts_match_their_generator() {
    charts_ready();
    let layout = Layout::new();
    let layout_path = corpus().join("charts/layout.json");
    let mut stale = Vec::new();
    if std::fs::read_to_string(&layout_path).ok() != Some(layout_json(&layout)) {
        stale.push(layout_path.display().to_string());
    }
    for spec in chart_specs() {
        let path = chart_path(&spec.name);
        if std::fs::read(&path).ok() != Some(generate(&spec, &layout)) {
            stale.push(path.display().to_string());
        }
    }
    assert!(
        stale.is_empty(),
        "Charts differ from the generator (rerun with RAWMAKASE_BLESS=1 after an intended change):\n{}",
        stale.join("\n")
    );
}

#[test]
fn charts_decode_to_their_camera_values() {
    let layout = Layout::new();
    for spec in chart_specs() {
        let im = develop(&chart_path(&spec.name));
        assert_eq!((im.width, im.height), (chart::WIDTH, chart::HEIGHT));
        let expected = chart::render(&layout, &spec.camera, spec.illuminant);
        let wb = im.metadata.wb;
        for p in layout.patches.iter().filter(|p| p.group != "sweep") {
            let i = ((p.y + p.h / 2) * chart::WIDTH + p.x + p.w / 2) as usize;
            for c in 0..3 {
                let gain = wb[c] / wb[1];
                let want = expected.camera[i][c] as f32 * gain;
                let got = im.pixels[i][c];
                // Half a 16-bit step, scaled by white balance, plus 0.1 %.
                assert!(
                    (got - want).abs() <= 0.6 / 65535. * gain + 1e-3 * want,
                    "{} {} channel {c}: decoded {got}, expected {want}",
                    spec.name,
                    p.name
                );
            }
        }
    }
}

const SNAPSHOT_MAX: f64 = 0.5;
const SNAPSHOT_MEAN: f64 = 0.1;

/// Catches any change in RAWmakase's colors: every chart and case against the
/// committed snapshot of RAWmakase's own output.
#[test]
fn colors_match_snapshots() {
    let layout = Layout::new();
    let cases = cases();
    let mut failures = Vec::new();
    for spec in chart_specs() {
        let path = corpus()
            .join("snapshots")
            .join(format!("{}.json", spec.name));
        let rendered = render_chart(&spec.name, &layout, &cases, &[], embedded_profiles, None);
        let mut current = BTreeMap::new();
        for (name, result) in rendered {
            match result {
                Ok(values) => {
                    current.insert(name, values);
                }
                Err(e) => failures.push(format!("{} / {name}: {e}", spec.name)),
            }
        }
        if bless() {
            PatchFile {
                about: BTreeMap::from([(
                    "what".into(),
                    "RAWmakase's own render of each case: mean encoded sRGB (16-bit) per patch, in layout.json order.".into(),
                )]),
                cases: current,
            }
            .write(&path);
            continue;
        }
        let snapshot = PatchFile::read(&path).unwrap_or_default();
        for name in snapshot.cases.keys().filter(|n| !current.contains_key(*n)) {
            failures.push(format!(
                "{} / {name}: snapshot has a case that no longer exists",
                spec.name
            ));
        }
        for (name, values) in &current {
            let Some(expected) = snapshot.cases.get(name) else {
                failures.push(format!("{} / {name}: no snapshot", spec.name));
                continue;
            };
            if expected.len() != layout.patches.len() || values.len() != layout.patches.len() {
                failures.push(format!(
                    "{} / {name}: snapshot has {} patches and the render {}, the layout {}",
                    spec.name,
                    expected.len(),
                    values.len(),
                    layout.patches.len()
                ));
                continue;
            }
            let c = compare(expected, values, |_| true);
            if c.max > SNAPSHOT_MAX || c.mean > SNAPSHOT_MEAN {
                let mut worst: Vec<usize> = (0..values.len()).collect();
                worst.sort_by(|a, b| c.delta_e[*b].total_cmp(&c.delta_e[*a]));
                let lines: Vec<String> = worst
                    .iter()
                    .take(8)
                    .map(|&i| {
                        format!(
                            "    {}",
                            describe(&layout.patches[i], expected[i], values[i], c.delta_e[i])
                        )
                    })
                    .collect();
                failures.push(format!(
                    "{} / {name}: mean ΔE00 {:.3}, max {:.2}\n{}",
                    spec.name,
                    c.mean,
                    c.max,
                    lines.join("\n")
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "Colors changed in {} cases. If intended, rerun with RAWMAKASE_BLESS=1 and commit the snapshot diff:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Exported JPEG and 16-bit TIFF files carry the same colors as the render.
#[test]
fn exports_match_the_render() {
    let layout = Layout::new();
    let cases = cases();
    let source = chart_path("synthetic-d65");
    let im = develop(&source);
    let profiles = embedded_profiles(&im);
    let dir = tempfile::tempdir().unwrap();
    for name in ["default", "saturation+50", "contrast+50"] {
        let case = cases.cases.iter().find(|c| c.name == name).unwrap();
        let out = render(&im, &profiles, &case.xmp(&cases.base, &[]), 0).unwrap();
        let rendered = measure::patches(out.width, &out.pixels, &layout.patches);
        // JPEG: 8-bit steps are up to about ΔE00 2 in the darkest patches.
        for (extension, mean, max) in [("tif", 0.01, 0.05), ("jpg", 0.3, 2.)] {
            let path = dir.path().join(format!("{name}.{extension}"));
            rawmakase::export::export(
                &path,
                &source,
                &out,
                &im.metadata,
                &rawmakase::export_settings::ExportOptions::default(),
                rawmakase::export::Replace::NoClobber,
            )
            .unwrap();
            let decoded = image::open(&path).unwrap().to_rgb16();
            let pixels: Vec<[f32; 3]> = decoded
                .pixels()
                .map(|p| p.0.map(|v| v as f32 / 65535.))
                .collect();
            let exported = measure::patches(decoded.width(), &pixels, &layout.patches);
            let c = compare(&rendered, &exported, |_| true);
            assert!(
                c.mean <= mean && c.max <= max,
                "{name}.{extension}: export differs from the render: mean ΔE00 {:.3}, max {:.3}",
                c.mean,
                c.max
            );
        }
    }
}

/// A downscaled render (as for the Fit preview) keeps the full render's colors.
#[test]
fn preview_size_matches_full_size() {
    let layout = Layout::new();
    let cases = cases();
    let im = develop(&chart_path("synthetic-d65"));
    let profiles = embedded_profiles(&im);
    for name in ["default", "contrast+100", "hue-blue+100"] {
        let case = cases.cases.iter().find(|c| c.name == name).unwrap();
        let xmp = case.xmp(&cases.base, &[]);
        let full = render(&im, &profiles, &xmp, 0).unwrap();
        let small = render(&im, &profiles, &xmp, 700).unwrap();
        let scale = small.width as f64 / full.width as f64;
        // Flat patches only, measured in the middle of the scaled area.
        let (full_patches, small_patches): (Vec<Patch>, Vec<Patch>) = layout
            .patches
            .iter()
            .filter(|p| p.group != "sweep")
            .map(|p| {
                let s = |v: u32| (v as f64 * scale).round() as u32;
                let scaled = Patch {
                    x: s(p.x) + 1,
                    y: s(p.y) + 1,
                    w: s(p.w).saturating_sub(2).max(1),
                    h: s(p.h).saturating_sub(2).max(1),
                    ..p.clone()
                };
                (p.clone(), scaled)
            })
            .unzip();
        let a = measure::patches(full.width, &full.pixels, &full_patches);
        let b = measure::patches(small.width, &small.pixels, &small_patches);
        let c = compare(&a, &b, |_| true);
        assert!(
            c.max <= 0.5,
            "{name}: preview differs by up to ΔE00 {:.3}",
            c.max
        );
    }
}

/// Per-area summaries of a comparison with Camera Raw.
pub struct Summary {
    pub delta_e: Comparison,
    /// Mean ΔL* on the gray ramp steps between −6 and +2 EV.
    pub tone: f64,
    /// Ratio of L* slopes against exposure on the ramp between −3 and +1.5 EV.
    pub contrast: f64,
    /// Chroma-weighted mean |Δh| on the hue grid, in degrees.
    pub hue: f64,
    /// Mean relative chroma difference on the hue grid, in percent.
    pub saturation: f64,
}

pub fn summarise(layout: &Layout, reference: &[[u16; 3]], candidate: &[[u16; 3]]) -> Summary {
    let patches = &layout.patches;
    let is = |i: usize, g: &str| patches[i].group == g;
    let delta_e = compare(reference, candidate, |i| !is(i, "wide"));
    let labs = |v: &[[u16; 3]]| v.iter().map(|p| lab(*p)).collect::<Vec<_>>();
    let (a, b) = (labs(reference), labs(candidate));
    let ramp: Vec<usize> = (0..patches.len()).filter(|&i| is(i, "ramp")).collect();
    let ev = |i: usize| -8. + 0.5 * ramp.iter().position(|&r| r == i).unwrap() as f64;
    let tone_steps: Vec<usize> = ramp
        .iter()
        .copied()
        .filter(|&i| (-6. ..=2.).contains(&ev(i)))
        .collect();
    let tone = tone_steps.iter().map(|&i| b[i][0] - a[i][0]).sum::<f64>() / tone_steps.len() as f64;
    let slope = |labs: &[[f64; 3]]| {
        let steps: Vec<usize> = ramp
            .iter()
            .copied()
            .filter(|&i| (-3. ..=1.5).contains(&ev(i)))
            .collect();
        let n = steps.len() as f64;
        let (mx, my) = (
            steps.iter().map(|&i| ev(i)).sum::<f64>() / n,
            steps.iter().map(|&i| labs[i][0]).sum::<f64>() / n,
        );
        steps
            .iter()
            .map(|&i| (ev(i) - mx) * (labs[i][0] - my))
            .sum::<f64>()
            / steps.iter().map(|&i| (ev(i) - mx).powi(2)).sum::<f64>()
    };
    let contrast = slope(&b) / slope(&a);
    let hues: Vec<usize> = (0..patches.len())
        .filter(|&i| is(i, "hue") && chroma(a[i]) > 5. && chroma(b[i]) > 5.)
        .collect();
    let weight: f64 = hues.iter().map(|&i| chroma(a[i])).sum();
    let hue = hues
        .iter()
        .map(|&i| hue_difference(a[i], b[i]).abs() * chroma(a[i]))
        .sum::<f64>()
        / weight.max(1e-9);
    let saturation = 100.
        * hues
            .iter()
            .map(|&i| chroma(b[i]) / chroma(a[i]) - 1.)
            .sum::<f64>()
        / hues.len().max(1) as f64;
    Summary {
        delta_e,
        tone,
        contrast,
        hue,
        saturation,
    }
}

/// Allowed worsening against the parity baseline before a test fails.
const PARITY_MEAN_MARGIN: f64 = 0.1;
const PARITY_P95_MARGIN: f64 = 0.3;
/// Cases whose committed Camera Raw reference is identical to that chart's
/// `default`, so the case measures nothing and its baseline pins a distance that
/// is really RAWmakase's own output. Re-render such a case with
/// `camera-raw-charts.py --charts <chart> --cases <case>` and drop its entry here
/// once it differs. (Camera Raw ignores Red, Green and Blue point curves unless
/// the master curve and all three are given, as Lightroom writes them.)
const UNMEASURED_REFERENCES: &[(&str, &str)] = &[];

#[derive(Default, Serialize, Deserialize)]
pub struct Baseline {
    #[serde(default)]
    pub about: String,
    /// chart → case → [mean ΔE00, p95 ΔE00]
    pub charts: BTreeMap<String, BTreeMap<String, [f64; 2]>>,
}

/// Compares renders with Camera Raw references and fails when any case is worse than
/// its baseline. Prints a table per chart with tone, contrast, hue and saturation.
pub fn check_parity(
    title: &str,
    references: &[(String, PatchFile)],
    baseline_path: &Path,
    extra: &[(&str, &str)],
    profiles: impl Fn(&CameraImage) -> Vec<Arc<CameraProfile>> + Copy,
) {
    let layout = Layout::new();
    let cases = cases();
    let mut baseline: Baseline = std::fs::read(baseline_path)
        .ok()
        .map(|b| serde_json::from_slice(&b).expect("baseline"))
        .unwrap_or_default();
    let mut failures = Vec::new();
    for (chart, reference) in references {
        // A reference identical to `default`'s records no Camera Raw behaviour, so
        // its ΔE00 is RAWmakase compared with itself and the baseline would keep it
        // there. See UNMEASURED_REFERENCES.
        if let Some(base) = reference.cases.get("default") {
            for (name, expected) in &reference.cases {
                if name != "default"
                    && expected == base
                    && !UNMEASURED_REFERENCES.contains(&(chart.as_str(), name.as_str()))
                {
                    failures.push(format!(
                        "{chart} / {name}: the Camera Raw reference is identical to default, so \
                         this case measures nothing. Re-render it with \
                         scripts/corpus/camera-raw-charts.py, or list it in UNMEASURED_REFERENCES."
                    ));
                }
            }
        }
        let current = render_chart(
            chart,
            &layout,
            &cases,
            extra,
            profiles,
            Some(&reference.cases),
        );
        // RAWmakase's patch values for scripts/corpus/parity-report.py.
        if let Some(dir) = std::env::var_os("RAWMAKASE_PARITY_DUMP") {
            PatchFile {
                about: BTreeMap::new(),
                cases: current
                    .iter()
                    .filter_map(|(name, v)| Some((name.clone(), v.as_ref().ok()?.clone())))
                    .collect(),
            }
            .write(&Path::new(&dir).join(format!("{chart}.json")));
        }
        println!(
            "\n{title}: {chart} (Camera Raw {})",
            reference.about.get("camera_raw").map_or("?", |s| s)
        );
        println!(
            "{:44} {:>6} {:>6} {:>7} {:>8} {:>6} {:>7}",
            "case", "ΔE00", "p95", "tone L*", "contrast", "hue °", "chroma%"
        );
        let entry = baseline.charts.entry(chart.clone()).or_default();
        for (name, expected) in &reference.cases {
            let values = match current.get(name) {
                Some(Ok(values)) => values,
                Some(Err(e)) => {
                    println!("{name:44} {e}");
                    failures.push(format!("{chart} / {name}: {e}"));
                    continue;
                }
                None => {
                    failures.push(format!(
                        "{chart} / {name}: reference exists but the case is not in cases.json"
                    ));
                    continue;
                }
            };
            let s = summarise(&layout, expected, values);
            println!(
                "{name:44} {:6.2} {:6.2} {:+7.2} {:8.3} {:6.2} {:+7.1}",
                s.delta_e.mean, s.delta_e.p95, s.tone, s.contrast, s.hue, s.saturation
            );
            if bless() {
                entry.insert(name.clone(), [round(s.delta_e.mean), round(s.delta_e.p95)]);
            } else if let Some([mean, p95]) = entry.get(name) {
                if s.delta_e.mean > mean + PARITY_MEAN_MARGIN
                    || s.delta_e.p95 > p95 + PARITY_P95_MARGIN
                {
                    failures.push(format!(
                        "{chart} / {name}: mean ΔE00 {:.2} (baseline {mean:.2}), p95 {:.2} (baseline {p95:.2})",
                        s.delta_e.mean, s.delta_e.p95
                    ));
                }
            } else {
                failures.push(format!("{chart} / {name}: no baseline"));
            }
        }
    }
    if bless() {
        baseline.about = "RAWmakase's distance from Camera Raw when last accepted: [mean, p95] CIEDE2000 per case, sRGB patches outside the wide-gamut row. Tests fail when a case gets worse.".into();
        std::fs::write(
            baseline_path,
            serde_json::to_string_pretty(&baseline).unwrap() + "\n",
        )
        .unwrap();
    }
    assert!(
        failures.is_empty(),
        "Further from Camera Raw than the baseline (rerun with RAWMAKASE_BLESS=1 to accept):\n{}",
        failures.join("\n")
    );
}

fn round(v: f64) -> f64 {
    (v * 1000.).round() / 1000.
}

/// Camera Raw references rendered from the committed charts with their embedded
/// matrices (no Adobe profiles), committed by scripts/corpus/camera-raw-charts.py.
#[test]
fn camera_raw_parity_does_not_regress() {
    let dir = corpus().join("camera-raw");
    let references: Vec<(String, PatchFile)> = chart_specs()
        .into_iter()
        .filter_map(|spec| {
            PatchFile::read(&dir.join(format!("{}.json", spec.name))).map(|f| (spec.name, f))
        })
        .collect();
    if references.is_empty() {
        println!("No Camera Raw references in {}", dir.display());
        return;
    }
    check_parity(
        "Embedded profile",
        &references,
        &dir.join("baseline.json"),
        &[],
        embedded_profiles,
    );
}

/// Auto black & white on the synthetic chart, which the Auto mix was not fitted to,
/// against Camera Raw 18.7's Auto for the same chart (ConvertToGrayscale and
/// AutoGrayscaleMix in a sidecar; Camera Raw stores the mix it resolved).
#[test]
fn auto_black_white_mix_matches_camera_raw_on_the_chart() {
    const CAMERA_RAW: [i32; 8] = [-9, -19, -23, -27, -18, 11, 16, 4];
    let im = develop(&chart_path("synthetic-d65"));
    let profiles = embedded_profiles(&im);
    let r = rawmakase::model::recipe::Recipe::with_profiles(&im.metadata, &profiles);
    let spread = rawmakase::develop::ColorSpread::measure(&im);
    let auto = rawmakase::develop::AutoMix {
        spread: &spread,
        metadata: &im.metadata,
    };
    let mix = auto.for_recipe(&r).map(|v| (v * 100.).round() as i32);
    let worst = mix
        .iter()
        .zip(CAMERA_RAW)
        .map(|(a, b)| (a - b).abs())
        .max()
        .unwrap();
    assert!(worst <= 3, "{mix:?} against Camera Raw's {CAMERA_RAW:?}");
}
