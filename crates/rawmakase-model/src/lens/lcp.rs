//! Adobe lens profiles (LCP) as a photo uses them. Users import them explicitly;
//! they are copied into `lens-profiles` under the data directory and never read
//! from an Adobe installation. The imported profiles that fit a photo's camera are
//! its choices for "Enable Profile Corrections" (`optics::lcp::PhotoProfiles`); the
//! one that fits its lens best is the automatic choice. The profiles' data and the
//! correction model are in `optics::lcp`; matching them to a photo is here.
use crate::camera_data::Metadata;
use crate::optics::lcp::{Candidate, Chromatic, Entry, ImportedProfile, PhotoProfiles, parse};
use crate::optics::{LensCorrection, Radial};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

impl Entry {
    /// Whether the entry describes any correction this reader applies for `m`,
    /// counting distortion it leaves to the camera's own data when the photo has it.
    fn has_model(&self, m: &Metadata) -> bool {
        self.distortion.is_some()
            || self.vignette.is_some()
            || (self.red.is_some() && self.blue.is_some())
            || (self.prefer_metadata_distortion
                && m.lens.as_ref().is_some_and(|l| l.distortion.is_some()))
    }
}

fn key(s: &str) -> String {
    s.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn lens_matches(e: &Entry, m: &Metadata) -> bool {
    let lens = key(&m.lens_model);
    !lens.is_empty() && e.lens.iter().any(|l| key(l) == lens)
}
/// The photo's crop factor, when its 35mm-equivalent focal length is recorded.
fn crop_factor(m: &Metadata) -> Option<f32> {
    (m.focal > 0. && m.focal_35mm > 0.).then(|| m.focal_35mm / m.focal)
}
/// How well the profile fits the photo's camera: 0 made on the same make, 1 on a
/// make sharing a lens mount, 2 on any other; None when the profile was made on a
/// smaller sensor and does not cover the photo. Adobe profiles
/// third-party lenses on one body per mount (a Sigma L-mount lens on a Sigma fp),
/// and Lightroom applies them to other makes too.
fn make_rank(e: &Entry, m: &Metadata) -> Option<u8> {
    const MOUNTS: [&[&str]; 2] = [
        &["sigma", "panasonic", "leica"],
        &["olympus", "om digital", "panasonic"],
    ];
    let (profile, camera) = (key(&e.make), key(&m.make));
    // Same make or not, a profile made on a smaller sensor does not cover this one
    // (a Micro Four Thirds profile on a full-frame Lumix).
    if crop_factor(m).is_some_and(|c| e.sensor_factor > c * 1.1) {
        return None;
    }
    if profile.is_empty() || profile == camera || camera.contains(&profile) {
        return Some(0);
    }
    Some(
        if MOUNTS.iter().any(|makes| {
            makes.iter().any(|k| profile.contains(k)) && makes.iter().any(|k| camera.contains(k))
        }) {
            1
        } else {
            2
        },
    )
}

/// Parameters interpolated in focal length (and APEX aperture for vignetting).
fn interpolate<T: Copy>(
    entries: &[&Entry],
    focal: f32,
    aperture: Option<f32>,
    get: impl Fn(&Entry) -> Option<T>,
    mix: impl Fn(T, T, f32) -> T,
) -> Option<(T, f32)> {
    let with: Vec<&&Entry> = entries.iter().filter(|e| get(e).is_some()).collect();
    let mut focals: Vec<f32> = with.iter().map(|e| e.focal).collect();
    focals.sort_by(f32::total_cmp);
    focals.dedup();
    let lo = focals
        .iter()
        .rev()
        .find(|f| **f <= focal)
        .or(focals.first())?;
    let hi = focals.iter().find(|f| **f >= focal).or(focals.last())?;
    let at_focal = |f: f32| -> Option<(T, f32)> {
        let mut here: Vec<&&&Entry> = with.iter().filter(|e| e.focal == f).collect();
        // Prefer the farthest focus distance, as Lightroom does without distance data.
        let far = here
            .iter()
            .map(|e| e.distance.unwrap_or(f32::INFINITY))
            .fold(0f32, f32::max);
        here.retain(|e| e.distance.unwrap_or(f32::INFINITY) == far);
        match aperture {
            Some(av) if here.iter().any(|e| e.aperture.is_some()) => {
                here.sort_by(|a, b| {
                    a.aperture
                        .unwrap_or(0.)
                        .total_cmp(&b.aperture.unwrap_or(0.))
                });
                let below = here
                    .iter()
                    .rev()
                    .find(|e| e.aperture.unwrap_or(0.) <= av)
                    .or(here.first())?;
                let above = here
                    .iter()
                    .find(|e| e.aperture.unwrap_or(0.) >= av)
                    .or(here.last())?;
                let (a0, a1) = (below.aperture.unwrap_or(av), above.aperture.unwrap_or(av));
                let t = if a1 > a0 { (av - a0) / (a1 - a0) } else { 0. };
                Some((mix(get(below)?, get(above)?, t), below.focal))
            }
            _ => here.first().and_then(|e| get(e).map(|v| (v, e.focal))),
        }
    };
    let (a, _) = at_focal(*lo)?;
    let (b, _) = at_focal(*hi)?;
    let t = if hi > lo {
        (focal - lo) / (hi - lo)
    } else {
        0.
    };
    Some((mix(a, b, t), focal))
}
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Which of a profile's entries describe the photo's lens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LensEntries {
    /// Only entries naming the photo's lens, as automatic matching uses.
    ThisLens,
    /// Every entry: a profile the user chose for another lens (an adapted or manual
    /// lens), as Lightroom's Custom setup allows.
    Any,
}

/// The correction for a photo from matching profile entries.
#[cfg(test)]
pub fn correction(entries: &[Entry], m: &Metadata) -> Option<LensCorrection> {
    correction_from(entries, m, LensEntries::ThisLens)
}
fn correction_from(entries: &[Entry], m: &Metadata, which: LensEntries) -> Option<LensCorrection> {
    let mut found: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.has_model(m) && (which == LensEntries::Any || lens_matches(e, m)))
        .collect();
    match (found.iter().filter_map(|e| make_rank(e, m)).min(), which) {
        (Some(best), _) => found.retain(|e| make_rank(e, m) == Some(best)),
        (None, LensEntries::ThisLens) => return None,
        (None, LensEntries::Any) => {}
    }
    if found.iter().any(|e| e.raw) {
        found.retain(|e| e.raw);
    }
    let first = *found.first()?;
    let focal = if m.focal > 0. { m.focal } else { first.focal };
    let av = (m.aperture > 0.).then(|| 2. * m.aperture.log2());
    // Convert our radius (1 = half diagonal) to the model's.
    let (w, h) = if m.crop_width > 0 && m.crop_height > 0 {
        (m.crop_width as f32, m.crop_height as f32)
    } else {
        (m.width as f32, m.height as f32)
    };
    let long = w.max(h);
    let half = (w * w + h * h).sqrt() * 0.5;
    // The model is normalised to the profiled sensor; rescale to this one when
    // their formats differ (a full-frame profile on an APS-C body).
    let format = crop_factor(m)
        .map(|c| c / first.sensor_factor)
        .filter(|r| (r.ln()).abs() > 1.1f32.ln())
        .unwrap_or(1.);
    let fx = (first.focal_x.unwrap_or(focal * first.sensor_factor / 36.) * format).max(1e-3);
    let to_model = half / (fx * long);
    let knots: Vec<f32> = (0..=32).map(|i| i as f32 / 32.).collect();
    let poly = |k: [f32; 3], r: f32| {
        let r2 = (r * to_model).powi(2);
        1. + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2
    };
    let curve = |f: &dyn Fn(f32) -> f32| Radial {
        knots: knots.clone(),
        values: knots.iter().map(|r| f(*r)).collect(),
    };
    let distortion = match &m.lens {
        Some(builtin) if first.prefer_metadata_distortion && builtin.distortion.is_some() => {
            builtin.distortion.clone()
        }
        _ => interpolate(&found, focal, None, |e| e.distortion, mix3)
            .map(|(k, _)| curve(&|r| poly(k, r))),
    };
    let vignetting = interpolate(&found, focal, av, |e| e.vignette, mix3)
        .map(|(k, _)| curve(&|r| 1. / poly(k, r).max(0.2)));
    let chroma = |get: fn(&Entry) -> Option<Chromatic>| {
        interpolate(&found, focal, None, get, |a, b, t| {
            (a.0 + (b.0 - a.0) * t, mix3(a.1, b.1, t))
        })
        .map(|((s, k), _)| curve(&|r| s * poly(k, r)))
    };
    let chromatic = chroma(|e| e.red)
        .zip(chroma(|e| e.blue))
        .map(|(r, b)| [r, b]);
    let c = LensCorrection {
        source: format!("Adobe profile: {}", first.name),
        default_on: false,
        vignetting,
        distortion,
        chromatic,
    };
    (!c.is_empty() && c.validate()).then_some(c)
}

pub fn library_dirs() -> Vec<PathBuf> {
    crate::storage::asset_dirs()
        .into_iter()
        .map(|p| p.join("lens-profiles"))
        .collect()
}

/// Every imported lens profile, read once and again when the folders change.
#[derive(Debug, Default)]
pub struct Library {
    profiles: Vec<Arc<ImportedProfile>>,
}
impl Library {
    /// Profiles from LCP texts, by file name; files that do not parse are skipped.
    pub fn from_texts<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut profiles: Vec<Arc<ImportedProfile>> = Vec::new();
        for (filename, text) in files {
            if profiles
                .iter()
                .any(|p| p.filename.eq_ignore_ascii_case(filename))
            {
                continue;
            }
            if let Ok(entries) = parse(text) {
                profiles.push(Arc::new(ImportedProfile::new(
                    filename.to_string(),
                    entries,
                )));
            }
        }
        Self { profiles }
    }
    fn load(dirs: &[PathBuf]) -> Self {
        let mut files: Vec<(String, String)> = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries.flatten().map(|f| f.path()).collect();
            paths.sort();
            for p in paths {
                if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lcp")) {
                    continue;
                }
                let (Some(name), Ok(text)) = (p.file_name(), std::fs::read_to_string(&p)) else {
                    continue;
                };
                files.push((name.to_string_lossy().into_owned(), text));
            }
        }
        Self::from_texts(files.iter().map(|(n, t)| (n.as_str(), t.as_str())))
    }
    /// The profiles a photo can use, Lightroom's Profile menus for its camera: those
    /// with an entry covering its sensor, raw profiles in place of non-raw ones for
    /// the same lens.
    pub fn for_photo(&self, m: &Metadata) -> PhotoProfiles {
        let fits: Vec<&Arc<ImportedProfile>> = self
            .profiles
            .iter()
            .filter(|p| {
                p.usable()
                    .any(|e| e.has_model(m) && make_rank(e, m).is_some())
            })
            .collect();
        // Collected once: with Adobe's whole library imported, over a thousand
        // profiles can fit one camera, and comparing each pair took seconds.
        let raw_lenses: HashSet<String> = fits
            .iter()
            .filter(|q| q.has_raw())
            .flat_map(|q| q.entries.iter().flat_map(|e| &e.lens))
            .map(|l| key(l))
            .collect();
        let shadowed = |p: &ImportedProfile| {
            !p.has_raw()
                && p.entries
                    .iter()
                    .flat_map(|e| &e.lens)
                    .any(|k| raw_lenses.contains(&key(k)))
        };
        let candidates: Vec<Candidate> = fits
            .iter()
            .filter(|p| !shadowed(p))
            .map(|p| Candidate {
                profile: Arc::clone(p),
                lens_rank: p
                    .usable()
                    // Entries of this lens without a correction model don't make it
                    // a profile of this lens.
                    .filter(|e| e.has_model(m) && lens_matches(e, m))
                    .filter_map(|e| make_rank(e, m))
                    .min(),
                correction: OnceLock::new(),
            })
            .collect();
        // Lightroom's menus list makes, models and profiles alphabetically; sorted
        // once here, not on every frame the panel is drawn.
        let mut menu: Vec<usize> = (0..candidates.len()).collect();
        menu.sort_by(|&a, &b| {
            let (a, b) = (&candidates[a].profile, &candidates[b].profile);
            (&a.lens_make, &a.lens_model, &a.name, &a.filename).cmp(&(
                &b.lens_make,
                &b.lens_model,
                &b.name,
                &b.filename,
            ))
        });
        PhotoProfiles {
            candidates: candidates.into(),
            menu: menu.into(),
        }
    }
}

/// What the library was read from: each folder and every LCP file in it, with
/// modification times and sizes, so a file replaced in place is read again.
type Stamp = Vec<(PathBuf, Option<SystemTime>, u64)>;
fn stamp(dirs: &[PathBuf]) -> Stamp {
    let mut out = Stamp::new();
    for dir in dirs {
        let modified = |m: &std::fs::Metadata| m.modified().ok();
        out.push((
            dir.clone(),
            std::fs::metadata(dir).ok().as_ref().and_then(modified),
            0,
        ));
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Stamp = entries
            .flatten()
            .filter(|f| {
                f.path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("lcp"))
            })
            .filter_map(|f| {
                let m = f.metadata().ok()?;
                Some((f.path(), modified(&m), m.len()))
            })
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

/// The imported profiles, read again only when a profile file changes.
#[derive(Default)]
pub struct LibraryCache {
    cached: Mutex<Option<(Stamp, Arc<Library>)>>,
}
impl LibraryCache {
    pub const fn new() -> Self {
        Self {
            cached: Mutex::new(None),
        }
    }
    /// The profiles as the files are now.
    pub fn current(&self) -> Arc<Library> {
        let dirs = library_dirs();
        let stamp = stamp(&dirs);
        if let Some((s, library)) = self.lock().as_ref()
            && *s == stamp
        {
            return Arc::clone(library);
        }
        // Read without the lock: a low-priority preview thread reading every profile
        // must not hold up Develop's open of a photo, which reads them itself instead.
        let library = Arc::new(Library::load(&dirs));
        *self.lock() = Some((stamp, Arc::clone(&library)));
        library
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(Stamp, Arc<Library>)>> {
        self.cached.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Candidate {
    /// The profile's correction for `m`, the photo it was listed for.
    pub fn correction(&self, m: &Metadata) -> Option<&LensCorrection> {
        self.correction
            .get_or_init(|| {
                let which = if self.lens_rank.is_some() {
                    LensEntries::ThisLens
                } else {
                    LensEntries::Any
                };
                correction_from(&self.profile.entries, m, which)
            })
            .as_ref()
    }
}

impl PhotoProfiles {
    /// Lightroom's automatic choice: a profile of the photo's lens, made on the same
    /// camera make, else on a make sharing the mount, else on any make.
    pub fn auto(&self, m: &Metadata) -> Option<&Candidate> {
        let mut matching: Vec<&Candidate> = self
            .candidates
            .iter()
            .filter(|c| c.lens_rank.is_some())
            .collect();
        matching.sort_by_key(|c| c.lens_rank);
        matching.into_iter().find(|c| c.correction(m).is_some())
    }
}
/// Lens profiles imported one by one, so a file that can't be used doesn't
/// stop the rest.
#[derive(Debug, Default)]
pub struct EachImported {
    pub imported: Vec<PathBuf>,
    /// Files left out, with why.
    pub refused: Vec<(PathBuf, String)>,
}
/// Validates and copies each lens profile into the data directory, going on
/// past the ones that can't be imported.
pub fn import_each(paths: &[PathBuf]) -> EachImported {
    import_each_into(paths, &crate::storage::data_dir().join("lens-profiles"))
}
fn import_each_into(paths: &[PathBuf], destination: &Path) -> EachImported {
    let mut out = EachImported::default();
    for path in paths {
        match import_into(std::slice::from_ref(path), destination) {
            Ok(done) => out.imported.extend(done),
            Err(e) => out.refused.push((path.clone(), format!("{e:#}"))),
        }
    }
    out
}
/// Validates and copies lens profiles into the data directory.
pub fn import_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    import_into(paths, &crate::storage::data_dir().join("lens-profiles"))
}
fn import_into(paths: &[PathBuf], destination: &Path) -> Result<Vec<PathBuf>> {
    use std::io::Write;
    ensure!(
        !paths.is_empty() && paths.len() <= 4096,
        "Choose 1–4096 lens profiles"
    );
    let mut staged = Vec::new();
    for path in paths {
        ensure!(
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("lcp")),
            "Choose LCP lens profiles: {}",
            path.display()
        );
        let bytes = std::fs::read(path)?;
        parse(std::str::from_utf8(&bytes)?).with_context(|| format!("{}", path.display()))?;
        let target = destination.join(path.file_name().context("Missing profile filename")?);
        if target.exists() {
            ensure!(
                std::fs::read(&target)? == bytes,
                "A different lens profile named {} is already imported",
                target.file_name().unwrap().to_string_lossy()
            );
        }
        staged.push((target, bytes));
    }
    std::fs::create_dir_all(destination)?;
    let mut imported = Vec::new();
    for (target, bytes) in staged {
        if !target.exists() {
            crate::storage::write_atomic(&target, crate::storage::Replace::NoClobber, |f| {
                Ok(f.write_all(&bytes)?)
            })?;
        }
        imported.push(target);
    }
    Ok(imported)
}

/// A synthetic raw LCP for one lens at 35mm, f/2: `distortion` and `vignette` are the
/// first radial parameters (k1, a1).
#[cfg(any(test, feature = "test-support"))]
pub fn test_profile(make: &str, lens: &str, name: &str, distortion: f32, vignette: f32) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
<photoshop:CameraProfiles><rdf:Seq>
<rdf:li><rdf:Description stCamera:Make="{make}" stCamera:CameraRawProfile="True" stCamera:Lens="{lens}"
 stCamera:LensPrettyName="{make} {lens}" stCamera:ProfileName="{name}" stCamera:SensorFormatFactor="1"
 stCamera:FocalLength="35" stCamera:ApertureValue="2">
 <stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="{distortion}">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="{vignette}"/>
 </rdf:Description></stCamera:PerspectiveModel></rdf:Description></rdf:li>
</rdf:Seq></photoshop:CameraProfiles></rdf:Description></rdf:RDF></x:xmpmeta>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
<photoshop:CameraProfiles><rdf:Seq>
<rdf:li><rdf:Description stCamera:Make="SONY" stCamera:CameraRawProfile="True" stCamera:Lens="FE 55mm F1.8 ZA"
 stCamera:LensPrettyName="Sony FE 55mm F1.8 ZA" stCamera:SensorFormatFactor="0.997786" stCamera:FocalLength="55"
 stCamera:FocusDistance="3" stCamera:ApertureValue="1.695994">
 <stCamera:AlternateLensNames><rdf:Seq><rdf:li>55mm F1.8 ZA</rdf:li></rdf:Seq></stCamera:AlternateLensNames>
 <stCamera:PerspectiveModel><rdf:Description stCamera:Version="2" stCamera:ScaleFactor="0.994929"
  stCamera:RadialDistortParam1="0.111952" stCamera:RadialDistortParam2="-0.511344" stCamera:RadialDistortParam3="-1.222533">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="-5.21124" stCamera:VignetteModelParam2="28.665015" stCamera:VignetteModelParam3="-89.161089"/>
 </rdf:Description></stCamera:PerspectiveModel></rdf:Description></rdf:li>
<rdf:li><rdf:Description stCamera:Make="SONY" stCamera:CameraRawProfile="True" stCamera:Lens="FE 55mm F1.8 ZA"
 stCamera:SensorFormatFactor="0.997786" stCamera:FocalLength="55" stCamera:FocusDistance="3" stCamera:ApertureValue="2">
 <stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0.111952" stCamera:RadialDistortParam2="-0.511344" stCamera:RadialDistortParam3="-1.222533">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="-2.871602" stCamera:VignetteModelParam2="-4.473586" stCamera:VignetteModelParam3="33.881413"/>
 </rdf:Description></stCamera:PerspectiveModel></rdf:Description></rdf:li>
</rdf:Seq></photoshop:CameraProfiles></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    #[test]
    fn one_unusable_file_does_not_stop_the_others() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("good.lcp");
        std::fs::write(&good, SAMPLE).unwrap();
        // Parses, but has no usable entry, as some of Adobe's own files.
        let empty = temp.path().join("empty.lcp");
        std::fs::write(
            &empty,
            SAMPLE.replace("FocalLength=\"55\"", "FocalLength=\"0\""),
        )
        .unwrap();
        let destination = temp.path().join("library");
        let paths = vec![empty.clone(), good];
        // All or nothing, as before.
        assert!(import_into(&paths, &destination).is_err());
        let done = import_each_into(&paths, &destination);
        assert_eq!(done.imported, [destination.join("good.lcp")]);
        assert_eq!(done.refused.len(), 1);
        assert_eq!(done.refused[0].0, empty);
        assert!(
            done.refused[0].1.contains("No usable lens profile entries"),
            "{}",
            done.refused[0].1
        );
    }
    fn a7ii(aperture: f32) -> Metadata {
        Metadata {
            make: "Sony".into(),
            model: "ILCE-7M2".into(),
            lens_model: "FE 55mm F1.8 ZA".into(),
            focal: 55.,
            aperture,
            width: 6000,
            height: 4000,
            ..Default::default()
        }
    }
    #[test]
    fn parses_and_evaluates_sony_profile() {
        let entries = parse(SAMPLE).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].lens, ["FE 55mm F1.8 ZA", "55mm F1.8 ZA"]);
        let c = correction(&entries, &a7ii(1.8)).unwrap();
        // Corner gain at f/1.8 computed independently from the LCP model: 1 / 0.5497.
        assert!(
            (c.vignetting_gain(1.) - 1.819).abs() < 0.01,
            "{}",
            c.vignetting_gain(1.)
        );
        assert!(c.vignetting_gain(0.) == 1.);
        let d = c.distortion.as_ref().unwrap();
        // The 55mm is almost distortion-free: under 0.1% at the corner.
        assert!((d.eval(1.) - 1.).abs() < 0.001);
        // Wider aperture numbers interpolate toward the f/2 entry.
        let f2 = correction(&entries, &a7ii(2.)).unwrap();
        assert!(f2.vignetting_gain(1.) < c.vignetting_gain(1.));
        let mut other = a7ii(1.8);
        other.lens_model = "FE 85mm F1.8".into();
        assert!(correction(&entries, &other).is_none());
        // A profile made on another camera make still applies, as in Lightroom.
        let mut other_make = a7ii(1.8);
        other_make.make = "Panasonic".into();
        let c2 = correction(&entries, &other_make).unwrap();
        assert_eq!(c2.vignetting_gain(1.), c.vignetting_gain(1.));
        let entry = |make: &str| Entry {
            make: make.into(),
            ..entries[0].clone()
        };
        let mut lumix = a7ii(1.8);
        lumix.make = "Panasonic".into();
        assert_eq!(make_rank(&entry("Panasonic"), &lumix), Some(0));
        assert_eq!(make_rank(&entry("SIGMA"), &lumix), Some(1));
        assert_eq!(make_rank(&entry("SONY"), &lumix), Some(2));
        assert_eq!(make_rank(&entry("SONY"), &a7ii(1.8)), Some(0));
        // A Micro Four Thirds profile does not cover a full-frame Lumix S.
        lumix.focal_35mm = 55.;
        let mft = Entry {
            sensor_factor: 2.,
            ..entry("OLYMPUS")
        };
        assert_eq!(make_rank(&mft, &lumix), None);
        let panasonic_mft = Entry {
            sensor_factor: 2.,
            ..entry("Panasonic")
        };
        assert_eq!(make_rank(&panasonic_mft, &lumix), None);
        // A full-frame profile on an APS-C body is scaled to the smaller sensor:
        // the APS-C corner sits at 2/3 of the full-frame radius.
        let mut aps_c = a7ii(1.8);
        aps_c.focal_35mm = 55. * 1.5;
        let crop = correction(&entries, &aps_c).unwrap();
        assert!((crop.vignetting_gain(1.) - c.vignetting_gain(2. / 3.)).abs() < 0.01);
        assert!(parse("<x/>").is_err());
    }
    #[test]
    fn reads_entries_written_as_parse_type_resource() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
<photoshop:CameraProfiles><rdf:Seq>
<rdf:li rdf:parseType="Resource">
<stCamera:Make>NIKON CORPORATION</stCamera:Make>
<stCamera:CameraRawProfile>True</stCamera:CameraRawProfile>
<stCamera:Lens>105.0 mm f/2.8</stCamera:Lens>
<stCamera:LensPrettyName>Nikon AF-S VR Micro-Nikkor 105mm f/2.8G IF-ED</stCamera:LensPrettyName>
<stCamera:ProfileName>Adobe (Nikon AF-S VR Micro-Nikkor 105mm f/2.8G IF-ED)</stCamera:ProfileName>
<stCamera:SensorFormatFactor>1.000000</stCamera:SensorFormatFactor>
<stCamera:FocalLength>105.000000</stCamera:FocalLength>
<stCamera:ApertureValue>2.970854</stCamera:ApertureValue>
<stCamera:PerspectiveModel rdf:parseType="Resource">
<stCamera:FocalLengthX>2.793782</stCamera:FocalLengthX>
<stCamera:RadialDistortParam1>-0.281840</stCamera:RadialDistortParam1>
<stCamera:RadialDistortParam2>2.421015</stCamera:RadialDistortParam2>
<stCamera:ChromaticRedGreenModel rdf:parseType="Resource">
<stCamera:ScaleFactor>0.999994</stCamera:ScaleFactor>
<stCamera:RadialDistortParam1>0.000811</stCamera:RadialDistortParam1>
</stCamera:ChromaticRedGreenModel>
<stCamera:ChromaticBlueGreenModel rdf:parseType="Resource">
<stCamera:ScaleFactor>0.999850</stCamera:ScaleFactor>
<stCamera:RadialDistortParam1>-0.004575</stCamera:RadialDistortParam1>
</stCamera:ChromaticBlueGreenModel>
<stCamera:VignetteModel rdf:parseType="Resource">
<stCamera:VignetteModelParam1>-23.421628</stCamera:VignetteModelParam1>
<stCamera:VignetteModelParam2>513.557706</stCamera:VignetteModelParam2>
<stCamera:VignetteModelParam3>-2930.260133</stCamera:VignetteModelParam3>
</stCamera:VignetteModel>
</stCamera:PerspectiveModel>
</rdf:li>
</rdf:Seq></photoshop:CameraProfiles></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let entries = parse(text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].lens, ["105.0 mm f/2.8"]);
        assert_eq!(
            entries[0].name,
            "Nikon AF-S VR Micro-Nikkor 105mm f/2.8G IF-ED"
        );
        assert!(entries[0].raw);
        assert!(entries[0].distortion.is_some());
        assert!(entries[0].vignette.is_some());
        assert!(entries[0].red.is_some() && entries[0].blue.is_some());
    }
    #[test]
    fn import_validates_and_copies() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.lcp");
        std::fs::write(&src, SAMPLE).unwrap();
        let dst = d.path().join("lib");
        assert_eq!(
            import_into(std::slice::from_ref(&src), &dst).unwrap().len(),
            1
        );
        assert!(import_into(&[src], &dst).is_ok());
        let bad = d.path().join("b.lcp");
        std::fs::write(&bad, "not xml").unwrap();
        assert!(import_into(&[bad], &dst).is_err());
    }
    #[test]
    fn library_stamp_follows_files_replaced_in_place() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("a.lcp");
        std::fs::write(&file, SAMPLE).unwrap();
        let dirs = [d.path().to_path_buf()];
        let before = stamp(&dirs);
        std::fs::write(&file, format!("{SAMPLE} ")).unwrap();
        assert_ne!(stamp(&dirs), before);
    }
}
