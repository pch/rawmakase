//! Adobe lens profiles (LCP) as a photo uses them. Users import them explicitly;
//! they are copied into `lens-profiles` under the data directory and never read
//! from an Adobe installation. The imported profiles that fit a photo's camera are
//! its choices for "Enable Profile Corrections" (`optics::lcp::PhotoProfiles`); the
//! one that fits its lens best is the automatic choice. The profiles' data and the
//! correction model are in `optics::lcp`; matching them to a photo is here.
use crate::camera_data::Metadata;
use crate::optics::lcp::{
    Candidate, Chromatic, Entry, ImportedProfile, PhotoProfiles, RadialModel, parse,
};
use crate::optics::{LensCorrection, Radial};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{HashMap, HashSet},
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
            || (self.prefer_metadata_distortion && camera_distortion(m).is_some())
    }
}

/// The distortion the camera stores, which a profile with PreferMetadataDistort keeps:
/// from its fuller data where the file has some (Sony's, of which Lightroom otherwise
/// applies only the lateral CA).
fn camera_distortion(m: &Metadata) -> Option<&crate::optics::Radial> {
    camera_lens(m)?.distortion.as_ref()
}
/// The centre of the camera's own correction, which its distortion is measured from.
fn camera_centre(m: &Metadata) -> [f32; 2] {
    camera_lens(m).map_or(LensCorrection::image_centre(), |l| l.centre)
}
fn camera_lens(m: &Metadata) -> Option<&LensCorrection> {
    (m.lens_stand_in.as_ref().or(m.lens.as_ref())).filter(|l| l.distortion.is_some())
}

fn key(s: &str) -> String {
    s.to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
/// Canon RAWs can spell the mount "EF 24-105mm" where an LCP says
/// "EF24-105mm". Only join EF to a following focal length: keep the rest
/// of the name, including mount and lens-version suffixes, distinct.
fn lens_key(s: &str) -> String {
    let mut name = key(s);
    if name
        .strip_prefix("ef ")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    {
        name.remove(2);
    }
    name
}
/// Whether an entry describes the photo's lens: it names the lens the file records,
/// or, for a file recording no lens at all, it carries the maker's lens ID. Nikon
/// bodies that write no lens model (the D600 among them) leave `stCamera:LensID`
/// and the maker note's `LensIDNumber` as the only identity, and they are the same
/// number.
fn lens_matches(e: &Entry, m: &Metadata) -> bool {
    let lens = lens_key(&m.lens_model);
    if !lens.is_empty() {
        return e.lens.iter().any(|l| lens_key(l) == lens);
    }
    lens_id_matches(e, m)
}
/// Whether an entry matches the photo's lens by the maker's lens ID alone, for a
/// file that records no lens name. Adobe's files hold the maker's own numbering
/// there, so another maker's profile never stands in for this camera's ID.
fn lens_id_matches(e: &Entry, m: &Metadata) -> bool {
    m.lens_model.is_empty()
        && m.lens_id != 0
        && e.lens_id == Some(m.lens_id)
        && maker(&e.make) == maker(&m.make)
        && lens_info_agrees(e, m)
}
/// Whether the entry's lens has the photo's focal range and maximum apertures, where
/// both say them: Nikon's lens IDs are not unique, and the lenses sharing one (an
/// 18-55mm f/3.5-5.6, a 17-35mm F4, a 50mm f/1.8) differ there. Adobe writes them to
/// a tenth, as the maker note gives them.
fn lens_info_agrees(e: &Entry, m: &Metadata) -> bool {
    match (e.lens_info, m.lens_info) {
        (Some(a), Some(b)) => a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.05),
        _ => true,
    }
}
/// A lens one profile file describes under one lens ID: its lens names (`lens_key`
/// of `stCamera:Lens` and AlternateLensNames, as one) and the display names its
/// entries give it (`Entry::name`, spacing and case aside; empty ones left out).
#[derive(Clone)]
struct FileLens {
    lens: String,
    display: HashSet<String>,
}
/// How many lenses these are: one file's lenses are as many as their lens names,
/// and two files' lenses of one lens name are one lens when they share a display
/// name.
fn lens_count(lenses: &[FileLens]) -> usize {
    let mut merged: Vec<FileLens> = Vec::new();
    for lens in lenses {
        let (same, other): (Vec<_>, Vec<_>) = merged
            .into_iter()
            .partition(|l| l.lens == lens.lens && !l.display.is_disjoint(&lens.display));
        let lens = same.into_iter().fold(lens.clone(), |mut a, b| {
            a.display.extend(b.display);
            a
        });
        merged = other;
        merged.push(lens);
    }
    merged.len()
}
/// The maker's first word, so Adobe's "NIKON CORPORATION" is the "Nikon" a camera
/// records: lens IDs only compare within one maker. Ricoh makes Pentax cameras and
/// profiles their lenses as "RICOH IMAGING COMPANY, LTD.", with Pentax's numbering.
fn maker(s: &str) -> String {
    match key(s).split(' ').next().unwrap_or("") {
        "ricoh" => "pentax".to_string(),
        first => first.to_string(),
    }
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
    // Convert our radius (1 = the decoded image's half diagonal) to the model's, whose
    // unit is a fraction of the photo's long edge, the camera's default crop.
    let cropped = m.crop_width > 0 && m.crop_height > 0;
    let (w, h) = if cropped {
        (m.crop_width as f32, m.crop_height as f32)
    } else {
        (m.width as f32, m.height as f32)
    };
    let long = w.max(h);
    let size = [m.width as f32, m.height as f32];
    let half = size[0].hypot(size[1]) * 0.5;
    // The profile's optical centre is a fraction of the photo; ours of the decoded
    // image. Distortion the camera keeps (PreferMetadataDistort) brings its own.
    let builtin = camera_distortion(m).filter(|_| first.prefer_metadata_distortion);
    let centre = match builtin {
        Some(_) => camera_centre(m),
        None => interpolate(
            &found,
            focal,
            None,
            |e| e.centre,
            |a, b, t| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t),
        )
        .map_or(LensCorrection::image_centre(), |(c, _)| {
            let (left, top) = if cropped {
                (m.crop_left as f32, m.crop_top as f32)
            } else {
                (0., 0.)
            };
            [(left + c[0] * w) / size[0], (top + c[1] * h) / size[1]]
        }),
    };
    // Put every model in our half-diagonal coordinate before interpolating.
    // Raw coefficients from different focal normalizations are not comparable:
    // k_n * (r * scale)^(2n) becomes (k_n * scale^(2n)) * r^(2n).
    // Use the calibration entry's focal length for missing FocalLengthX, including
    // at clamped endpoints, never the requested photo focal length.
    let normalized = |e: &Entry, model: RadialModel| {
        let format = crop_factor(m)
            .map(|c| c / e.sensor_factor)
            .filter(|r| (r.ln()).abs() > 1.1f32.ln())
            .unwrap_or(1.);
        let fx = (model.focal_x.unwrap_or(e.focal * e.sensor_factor / 36.) * format).max(1e-3);
        let scale2 = (half / (fx * long)).powi(2);
        [
            model.params[0] * scale2,
            model.params[1] * scale2 * scale2,
            model.params[2] * scale2 * scale2 * scale2,
        ]
    };
    let reach = crate::optics::reach(centre, size);
    let knots: Vec<f32> = (0..=32).map(|i| i as f32 / 32. * reach).collect();
    let poly = |k: [f32; 3], r: f32| {
        let r2 = r * r;
        1. + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2
    };
    let curve = |f: &dyn Fn(f32) -> f32| Radial {
        knots: knots.clone(),
        values: knots.iter().map(|r| f(*r)).collect(),
    };
    let distortion = match builtin {
        Some(builtin) => Some(builtin.clone()),
        None => interpolate(
            &found,
            focal,
            None,
            |e| e.distortion.map(|p| normalized(e, p)),
            mix3,
        )
        .map(|(k, _)| curve(&|r| poly(k, r))),
    };
    let vignetting = interpolate(
        &found,
        focal,
        av,
        |e| e.vignette.map(|p| normalized(e, p)),
        mix3,
    )
    .map(|(k, _)| curve(&|r| 1. / poly(k, r).max(0.2)));
    let chroma = |get: fn(&Entry) -> Option<Chromatic>| {
        interpolate(
            &found,
            focal,
            None,
            |e| get(e).map(|(s, p)| (s, normalized(e, p))),
            |a, b, t| (a.0 + (b.0 - a.0) * t, mix3(a.1, b.1, t)),
        )
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
        chromatic_at_source: false,
        gain_map: None,
        centre,
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
    /// The lens IDs that cannot say which lens a photo used, because the profiles
    /// carrying them describe more than one lens. Adobe's files hold the maker's own
    /// numbering, and it is not unique: Nikon's 165 is the 105mm f/1.4E, the 35mm
    /// f/1.8G and the 28-300mm f/3.5-5.6 alike, and a photo leaves no way to tell
    /// them apart. Lenses are told apart by their names, as Adobe's files give them
    /// (`stCamera:Lens`); RAW and non-RAW files of one lens name the same lens, so
    /// they stay usable. Those names can be generic ("18.0-200.0 mm f/3.5-5.6") and
    /// two generations' alike (Nikon's 139, the 18-200mm VR and VR II), so two files'
    /// lenses of one name are one lens only when they also share a display name;
    /// within one file they are one lens whatever its entries' display names, which
    /// Adobe spells two ways ("58mm" and "58.0mm"). Lenses whose focal range and
    /// apertures differ from the photo's (`lens_info_agrees`) don't count: they
    /// aren't the photo's lens.
    fn shared_lens_ids(&self, m: &Metadata) -> HashSet<u32> {
        let mut lenses: HashMap<u32, Vec<FileLens>> = HashMap::new();
        for p in &self.profiles {
            let mut file: HashMap<(u32, String), HashSet<String>> = HashMap::new();
            for e in &p.entries {
                let Some(id) = e.lens_id else { continue };
                if maker(&e.make) != maker(&m.make) || !lens_info_agrees(e, m) {
                    continue;
                }
                let lens = e
                    .lens
                    .iter()
                    .map(|l| lens_key(l))
                    .collect::<Vec<_>>()
                    .join("|");
                let display = file.entry((id, lens)).or_default();
                let name = e.name.split_whitespace().collect::<String>().to_lowercase();
                if !name.is_empty() {
                    display.insert(name);
                }
            }
            for ((id, lens), display) in file {
                lenses
                    .entry(id)
                    .or_default()
                    .push(FileLens { lens, display });
            }
        }
        lenses
            .into_iter()
            .filter(|(_, lenses)| lens_count(lenses) > 1)
            .map(|(id, _)| id)
            .collect()
    }
    /// The profiles a photo can use, Lightroom's Profile menus for its camera: those
    /// with an entry covering its sensor, raw profiles in place of non-raw ones for
    /// the same lens.
    pub fn for_photo(&self, m: &Metadata) -> PhotoProfiles {
        // An entry that matches only by the maker's lens ID identifies the lens when
        // that ID picks out one lens among the profiles this maker's files describe.
        let shared = self.shared_lens_ids(m);
        let identifies = |e: &Entry| {
            lens_matches(e, m) && !(shared.contains(&m.lens_id) && lens_id_matches(e, m))
        };
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
            .map(|l| lens_key(l))
            .collect();
        let shadowed = |p: &ImportedProfile| {
            !p.has_raw()
                && p.entries
                    .iter()
                    .flat_map(|e| &e.lens)
                    .any(|k| raw_lenses.contains(&lens_key(k)))
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
                    .filter(|e| e.has_model(m) && identifies(e))
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
    /// The lens of a photo that records only the maker's lens ID, as the imported
    /// profile identifying it names it (`stCamera:Lens`, the name Camera Raw records
    /// for the lens); `None` when the file names its lens or no profile identifies
    /// it. Pentax bodies write no lens name, only the ID.
    pub fn lens_by_id(&self, m: &Metadata) -> Option<&str> {
        if !m.lens_model.is_empty() || m.lens_id == 0 {
            return None;
        }
        // A candidate ranks for the lens only when an entry identifies it, which for
        // a file without a lens name means by an ID no other lens carries.
        let mut matching: Vec<&Candidate> = self
            .candidates
            .iter()
            .filter(|c| c.lens_rank.is_some())
            .collect();
        matching.sort_by_key(|c| c.lens_rank);
        matching
            .into_iter()
            .flat_map(|c| c.profile.usable())
            .filter(|e| lens_id_matches(e, m))
            .find_map(|e| e.lens.first())
            .map(String::as_str)
    }
}
/// The name Camera Raw gives a lens that a file describes only by its focal range
/// and maximum apertures (`Metadata::lens_info`), as Adobe's profiles of such lenses
/// hold it in `stCamera:Lens`: "18.0-55.0 mm f/3.5-5.6", "35.0 mm f/1.8".
pub fn lens_info_name(info: [f32; 4]) -> String {
    let [short, long, wide, tele] = info;
    let focal = if short == long {
        format!("{short:.1} mm")
    } else {
        format!("{short:.1}-{long:.1} mm")
    };
    if wide == tele {
        format!("{focal} f/{wide:.1}")
    } else {
        format!("{focal} f/{wide:.1}-{tele:.1}")
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
    test_profile_with_lens_id(make, lens, name, None, distortion, vignette)
}
/// `test_profile` with the profile's `stCamera:LensID` set, as Adobe writes it:
/// Nikon's `LensIDNumber` ("138") or Pentax's `LensType` bytes ("4 252").
#[cfg(any(test, feature = "test-support"))]
pub fn test_profile_with_lens_id(
    make: &str,
    lens: &str,
    name: &str,
    lens_id: Option<&str>,
    distortion: f32,
    vignette: f32,
) -> String {
    let id = lens_id.map_or_else(String::new, |v| format!(" stCamera:LensID=\"{v}\""));
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">
<photoshop:CameraProfiles><rdf:Seq>
<rdf:li><rdf:Description stCamera:Make="{make}" stCamera:CameraRawProfile="True" stCamera:Lens="{lens}"{id}
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
    /// The profile's optical centre is a fraction of the photo, the default crop; the
    /// correction's of the decoded image. Distortion the camera keeps keeps its centre.
    #[test]
    fn a_profiles_optical_centre_is_kept() {
        let text = SAMPLE.replace(
            "stCamera:RadialDistortParam1=\"0.111952\"",
            "stCamera:ImageXCenter=\"0.46\" stCamera:ImageYCenter=\"0.52\" stCamera:RadialDistortParam1=\"0.111952\"",
        );
        let entries = parse(&text).unwrap();
        assert_eq!(entries[0].centre, Some([0.46, 0.52]));
        let mut m = a7ii(1.8);
        m.width = 6048;
        m.height = 4024;
        m.crop_left = 8;
        m.crop_top = 4;
        m.crop_width = 6000;
        m.crop_height = 4000;
        let c = correction(&entries, &m).unwrap();
        let [x, y] = c.centre_in([6048., 4024.]);
        assert!(
            (x - 2768.).abs() < 0.01 && (y - 2084.).abs() < 0.01,
            "{x} {y}"
        );
        // The knots reach the decoded image's corner farthest from it.
        let far = crate::optics::reach(c.centre, [6048., 4024.]);
        assert!(*c.vignetting.as_ref().unwrap().knots.last().unwrap() >= far - 1e-5);
        // Without a centre, the image's.
        assert_eq!(parse(SAMPLE).unwrap()[0].centre, None);
        assert_eq!(
            correction(&parse(SAMPLE).unwrap(), &m).unwrap().centre,
            [0.5, 0.5]
        );
    }
    #[test]
    fn a_lens_id_matches_when_the_file_records_no_lens() {
        // A D600 writes no lens model, leaving the maker note's LensIDNumber, which
        // is also the profile's stCamera:LensID, as the only lens identity.
        let text = test_profile_with_lens_id(
            "NIKON CORPORATION",
            "105.0 mm f/2.8",
            "Adobe (Nikon AF-S VR Micro-Nikkor 105mm f/2.8G IF-ED)",
            Some("138"),
            -0.28,
            -0.5,
        );
        assert_eq!(parse(&text).unwrap()[0].lens_id, Some(138));
        let library = Library::from_texts([("105.lcp", text.as_str())]);
        let d600 = |lens_model: &str, lens_id: u32| {
            let mut m = Metadata {
                make: "Nikon".into(),
                model: "D600".into(),
                lens_model: lens_model.into(),
                lens_id,
                focal: 35.,
                aperture: 2.,
                width: 6034,
                height: 4028,
                ..Default::default()
            };
            m.lens_profiles = library.for_photo(&m);
            m
        };
        let unnamed = d600("", 138);
        assert_eq!(
            unnamed
                .lens_profiles
                .auto(&unnamed)
                .map(|c| c.profile.filename.as_str()),
            Some("105.lcp")
        );
        // Another lens' ID does not stand in for it.
        let other = d600("", 39);
        assert!(other.lens_profiles.auto(&other).is_none());
        // A file that does record a lens still matches by name, not by ID.
        let named = d600("105.0 mm f/2.8", 39);
        assert_eq!(
            named
                .lens_profiles
                .auto(&named)
                .map(|c| c.profile.filename.as_str()),
            Some("105.lcp")
        );
    }
    #[test]
    fn a_shared_lens_id_is_not_matched_automatically() {
        // Adobe's files hold the maker's own lens numbering, and Nikon's is not
        // unique: 165 is the 105mm f/1.4E and the 35mm f/1.8G alike, so an ID on its
        // own cannot say which lens took the photo.
        let nikon = |lens: &str, id: u32| {
            test_profile_with_lens_id(
                "NIKON CORPORATION",
                lens,
                &format!("Adobe (Nikon {lens})"),
                Some(&id.to_string()),
                -0.1,
                -0.5,
            )
        };
        let f14 = nikon("105.0 mm f/1.4", 165);
        let f18 = nikon("35.0 mm f/1.8", 165);
        let micro = nikon("105.0 mm f/2.8", 138);
        // Another maker numbers its own lenses: its 138 says nothing about this camera.
        let canon = test_profile_with_lens_id(
            "Canon",
            "50.0 mm f/1.4",
            "Adobe (Canon 50mm f/1.4)",
            Some("138"),
            -0.1,
            -0.5,
        );
        let library = Library::from_texts([
            ("f14.lcp", f14.as_str()),
            ("f18.lcp", f18.as_str()),
            ("micro.lcp", micro.as_str()),
            ("canon.lcp", canon.as_str()),
        ]);
        let photo = |lens_id: u32| {
            let mut m = Metadata {
                make: "Nikon".into(),
                model: "D600".into(),
                lens_id,
                focal: 35.,
                aperture: 2.,
                width: 6034,
                height: 4028,
                ..Default::default()
            };
            m.lens_profiles = library.for_photo(&m);
            m
        };
        // Two lenses to choose from: neither is taken, but both stay in the menus.
        let shared = photo(165);
        assert_eq!(shared.lens_profiles.all().len(), 4);
        assert!(shared.lens_profiles.auto(&shared).is_none());
        // A lens ID only this maker's one profile carries is the automatic choice.
        let unique = photo(138);
        assert_eq!(
            unique
                .lens_profiles
                .auto(&unique)
                .map(|c| c.profile.filename.as_str()),
            Some("micro.lcp")
        );
    }
    #[test]
    fn a_shared_nikon_lens_id_is_told_apart_by_the_lens_focal_range_and_apertures() {
        // Discussion #731: a D3100 with the AF-S DX VR 18-55mm writes no lens model,
        // only LensIDNumber 154, which Adobe's profiles also give a Tokina 17-35mm F4
        // and a Yongnuo 50mm f/1.8. The maker note's Lens tag (18-55mm f/3.5-5.6) is
        // the profiles' stCamera:LensInfo, and tells them apart.
        let nikon = |file: &str, lens: &str, id: u32, info: &str| {
            let text = test_profile_with_lens_id(
                "NIKON CORPORATION",
                lens,
                &format!("Adobe ({file})"),
                Some(&id.to_string()),
                -0.1,
                -0.5,
            )
            .replace(
                " stCamera:LensPrettyName",
                &format!(" stCamera:LensInfo=\"{info}\" stCamera:LensPrettyName"),
            );
            (format!("{file}.lcp"), text)
        };
        let files = [
            nikon(
                "vr",
                "18.0-55.0 mm f/3.5-5.6",
                154,
                "180/10 550/10 35/10 56/10",
            ),
            nikon(
                "tokina",
                "Tokina atx-i 17-35mm F4 FF",
                154,
                "170/10 350/10 40/10 40/10",
            ),
            nikon("yongnuo", "50.0 mm f/1.8", 154, "500/10 500/10 18/10 18/10"),
            // Another 18-55mm, by another ID.
            nikon(
                "ed-ii",
                "18.0-55.0 mm f/3.5-5.6",
                148,
                "180/10 550/10 35/10 56/10",
            ),
        ];
        assert_eq!(
            parse(&files[0].1).unwrap()[0].lens_info,
            Some([18., 55., 3.5, 5.6])
        );
        let library = Library::from_texts(files.iter().map(|(n, t)| (n.as_str(), t.as_str())));
        let d3100 = |lens_info: Option<[f32; 4]>| {
            let mut m = Metadata {
                make: "Nikon".into(),
                model: "D3100".into(),
                lens_id: 154,
                lens_info,
                focal: 35.,
                aperture: 2.,
                width: 4608,
                height: 3072,
                ..Default::default()
            };
            m.lens_profiles = library.for_photo(&m);
            m
        };
        let auto = |m: &Metadata| m.lens_profiles.auto(m).map(|c| c.profile.filename.clone());
        let kit = d3100(Some([18., 55., 3.5, 5.6]));
        assert_eq!(auto(&kit).as_deref(), Some("vr.lcp"));
        assert_eq!(
            kit.lens_profiles.lens_by_id(&kit),
            Some("18.0-55.0 mm f/3.5-5.6")
        );
        assert_eq!(
            auto(&d3100(Some([50., 50., 1.8, 1.8]))).as_deref(),
            Some("yongnuo.lcp")
        );
        // A lens none of them is, or a file that doesn't say: the ID can't tell.
        assert_eq!(auto(&d3100(Some([24., 70., 2.8, 2.8]))), None);
        assert_eq!(auto(&d3100(None)), None);
    }
    #[test]
    fn lenses_sharing_an_id_and_focal_range_stay_a_manual_choice() {
        // Adobe's two 18-200mm generations are both Nikon's 139, 18-200mm f/3.5-5.6,
        // and "18.0-200.0 mm f/3.5-5.6": only their display names tell them apart.
        // Raw and non-raw files of one lens (its name's case aside) are one lens.
        let nikon = |pretty: &str, raw: bool| {
            test_profile_with_lens_id(
                "NIKON CORPORATION",
                "18.0-200.0 mm f/3.5-5.6",
                &format!("Adobe ({pretty})"),
                Some("139"),
                -0.1,
                -0.5,
            )
            .replace(
                r#"stCamera:LensPrettyName="NIKON CORPORATION 18.0-200.0 mm f/3.5-5.6""#,
                &format!(
                    r#"stCamera:LensInfo="180/10 2000/10 35/10 56/10" stCamera:LensPrettyName="{pretty}""#
                ),
            )
            .replace(
                r#"CameraRawProfile="True""#,
                &format!(r#"CameraRawProfile="{}""#, if raw { "True" } else { "False" }),
            )
        };
        let vr = nikon(
            "Nikon AF-S DX VR Zoom-Nikkor 18-200mm f/3.5-5.6G IF-ED",
            true,
        );
        let vr_jpeg = nikon(
            "Nikon AF-S DX VR Zoom-NIKKOR 18-200mm f/3.5-5.6G IF-ED",
            false,
        );
        let vr_ii = nikon("Nikon AF-S DX NIKKOR 18-200mm f/3.5-5.6G ED VR II", true);
        assert_eq!(
            parse(&vr).unwrap()[0].name,
            "Nikon AF-S DX VR Zoom-Nikkor 18-200mm f/3.5-5.6G IF-ED"
        );
        let photo = |library: &Library| {
            let mut m = Metadata {
                make: "Nikon".into(),
                model: "D90".into(),
                lens_id: 139,
                lens_info: Some([18., 200., 3.5, 5.6]),
                focal: 35.,
                aperture: 2.,
                width: 4288,
                height: 2848,
                ..Default::default()
            };
            m.lens_profiles = library.for_photo(&m);
            m
        };
        let auto = |m: &Metadata| m.lens_profiles.auto(m).map(|c| c.profile.filename.clone());
        let both = Library::from_texts([("vr.lcp", vr.as_str()), ("vr-ii.lcp", vr_ii.as_str())]);
        assert_eq!(auto(&photo(&both)), None);
        // One file's entries may spell the name two ways ("58mm", "58.0mm" in Adobe's
        // 58mm f/1.4G): still one lens.
        let entry = |text: &str| {
            let start = text.find("<rdf:li>").unwrap();
            let end = text.find("</rdf:li>").unwrap() + "</rdf:li>".len();
            text[start..end].to_string()
        };
        let other = nikon(
            "Nikon AF-S DX DX VR Zoom-Nikkor 18-200mm f/3.5-5.6G IF-ED",
            true,
        )
        .replace(r#"FocalLength="35""#, r#"FocalLength="50""#);
        let two = vr.replace(&entry(&vr), &(entry(&vr) + &entry(&other)));
        assert_eq!(parse(&two).unwrap().len(), 2);
        let one_file = Library::from_texts([("vr.lcp", two.as_str())]);
        assert_eq!(auto(&photo(&one_file)).as_deref(), Some("vr.lcp"));
        let one = Library::from_texts([("vr.lcp", vr.as_str()), ("vr-jpeg.lcp", vr_jpeg.as_str())]);
        assert_eq!(auto(&photo(&one)).as_deref(), Some("vr.lcp"));
    }
    #[test]
    fn profiles_without_a_lens_display_name_stay_apart() {
        // Profiles made with Adobe's Lens Profile Creator may name no lens, or give
        // each the same profile name: lenses their names differ in stay two lenses.
        let profile = |lens: &str, pretty: Option<&str>, name: &str| {
            let text =
                test_profile_with_lens_id("NIKON CORPORATION", lens, name, Some("165"), -0.1, -0.5);
            let generated = format!(r#" stCamera:LensPrettyName="NIKON CORPORATION {lens}""#);
            match pretty {
                Some(p) => text.replace(&generated, &format!(r#" stCamera:LensPrettyName="{p}""#)),
                None => text.replace(&generated, ""),
            }
        };
        let photo = |library: &Library| {
            let mut m = Metadata {
                make: "Nikon".into(),
                model: "D600".into(),
                lens_id: 165,
                focal: 35.,
                aperture: 2.,
                width: 6034,
                height: 4028,
                ..Default::default()
            };
            m.lens_profiles = library.for_photo(&m);
            m
        };
        for (a, b) in [
            (
                profile("35.0 mm f/1.8", None, ""),
                profile("105.0 mm f/1.4", None, ""),
            ),
            (
                profile("35.0 mm f/1.8", Some(""), "A"),
                profile("105.0 mm f/1.4", Some(""), "B"),
            ),
            (
                profile("35.0 mm f/1.8", None, "Custom"),
                profile("105.0 mm f/1.4", None, "Custom"),
            ),
        ] {
            let library = Library::from_texts([("a.lcp", a.as_str()), ("b.lcp", b.as_str())]);
            let m = photo(&library);
            assert!(m.lens_profiles.auto(&m).is_none());
            assert_eq!(m.lens_profiles.lens_by_id(&m), None);
        }
        // Two lenses in one file, in either order, are two lenses too.
        let entry = |text: &str| {
            let start = text.find("<rdf:li>").unwrap();
            let end = text.find("</rdf:li>").unwrap() + "</rdf:li>".len();
            text[start..end].to_string()
        };
        let f18 = profile("35.0 mm f/1.8", None, "Custom");
        let f14 = profile("105.0 mm f/1.4", None, "Custom").replace("-0.1", "-0.2");
        for (a, b) in [(&f18, &f14), (&f14, &f18)] {
            let combined = a.replace(&entry(a), &(entry(a) + &entry(b)));
            assert_eq!(parse(&combined).unwrap().len(), 2);
            let library = Library::from_texts([("combined.lcp", combined.as_str())]);
            let m = photo(&library);
            assert!(m.lens_profiles.auto(&m).is_none());
            assert_eq!(m.lens_profiles.lens_by_id(&m), None);
        }
    }
    #[test]
    fn a_lens_described_by_its_focal_range_is_named_as_camera_raw_names_it() {
        assert_eq!(
            lens_info_name([18., 55., 3.5, 5.6]),
            "18.0-55.0 mm f/3.5-5.6"
        );
        assert_eq!(lens_info_name([35., 35., 1.8, 1.8]), "35.0 mm f/1.8");
        assert_eq!(lens_info_name([24., 120., 4., 4.]), "24.0-120.0 mm f/4.0");
    }
    #[test]
    fn a_pentax_lens_type_identifies_and_names_the_lens() {
        // Pentax bodies write no lens model (a K-x DNG among them, issue #501), only
        // the maker note's two-byte LensType, which LibRaw reads as one number and
        // Adobe's Pentax profiles write as stCamera:LensID "4 252". Profiles of newer
        // lenses are made by Ricoh, which makes Pentax cameras.
        let pentax = |make: &str, lens: &str, id: &str| {
            test_profile_with_lens_id(make, lens, &format!("Adobe ({lens})"), Some(id), -0.1, -0.5)
        };
        let da = pentax("PENTAX", "smc PENTAX-DA 18-55mm F3.5-5.6 AL", "4 252");
        let hd = pentax(
            "RICOH IMAGING COMPANY, LTD.",
            "HD PENTAX-DA 16-85mm F3.5-5.6 ED DC WR",
            "8 236",
        );
        assert_eq!(parse(&da).unwrap()[0].lens_id, Some(4 << 8 | 252));
        let library = Library::from_texts([("da.lcp", da.as_str()), ("hd.lcp", hd.as_str())]);
        let photo = |lens_id: u32| {
            let mut m = Metadata {
                make: "Pentax".into(),
                model: "K-x".into(),
                lens_id,
                focal: 35.,
                aperture: 2.,
                width: 4288,
                height: 2848,
                ..Default::default()
            };
            m.lens_profiles = library.for_photo(&m);
            m
        };
        let auto = |m: &Metadata| m.lens_profiles.auto(m).map(|c| c.profile.filename.clone());
        let lens = |m: &Metadata| m.lens_profiles.lens_by_id(m).map(str::to_string);
        let k_x = photo(4 << 8 | 252);
        assert_eq!(auto(&k_x).as_deref(), Some("da.lcp"));
        assert_eq!(
            lens(&k_x).as_deref(),
            Some("smc PENTAX-DA 18-55mm F3.5-5.6 AL")
        );
        let k_1 = photo(8 << 8 | 236);
        assert_eq!(auto(&k_1).as_deref(), Some("hd.lcp"));
        // A lens no imported profile carries stays unnamed.
        let sigma = photo(3 << 8 | 255);
        assert_eq!(auto(&sigma), None);
        assert_eq!(lens(&sigma), None);
        // Named so, it matches the same profile, with the same correction, by name.
        let mut named = photo(4 << 8 | 252);
        named.lens_model = lens(&k_x).unwrap();
        named.lens_profiles = library.for_photo(&named);
        assert_eq!(auto(&named).as_deref(), Some("da.lcp"));
        let gain = |m: &Metadata| {
            m.lens_profiles
                .auto(m)?
                .correction(m)
                .map(|c| c.vignetting_gain(1.))
        };
        assert_eq!(gain(&named), gain(&k_x));
        // A file that records its lens keeps the camera's name.
        assert_eq!(lens(&named), None);
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

#[cfg(test)]
#[path = "lcp_normalization_tests.rs"]
mod normalization_tests;
