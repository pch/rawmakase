//! The camera-profile catalogue: reviewed DCPs from pinned sources, which the user
//! downloads explicitly. `data/camera-profiles/sources.toml` (hand-written) says
//! where each source's files are and how to credit it; `catalogue.toml`, made by
//! [`generate`] from checkouts of those revisions, pins each profile's bytes,
//! camera and terms. Only this metadata ships. Matching, verification and the
//! no-clobber install stay here; the application supplies the transport.
use super::{CameraProfile, LutSpace, camera_fits, from_bytes, library};
use crate::camera_data::Metadata;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

const SOURCES: &str = include_str!("../../../../data/camera-profiles/sources.toml");
const CATALOGUE: &str = include_str!("../../../../data/camera-profiles/catalogue.toml");
/// The largest profile the transport reads: the importer's limit.
pub const MAX_BYTES: u64 = 16_000_000;

/// Where a source's files are, and how the app credits them.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub homepage: String,
    pub credit: String,
    pub repository: String,
    pub revision: String,
    pub directory: String,
    pub repository_license: String,
    pub repository_license_file: String,
}

/// What a profile's own notice lets anyone do with it. Only dedications are
/// recognised: a bare copyright notice, no notice, or the license of the
/// repository holding the file grant nothing here.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Terms {
    #[serde(rename = "public-domain")]
    PublicDomain,
    #[serde(rename = "CC0-1.0")]
    Cc0,
}

impl Terms {
    /// The terms a DCP's ProfileCopyright states, if any.
    pub fn of_notice(notice: &str) -> Option<Self> {
        let notice = notice.trim();
        if notice.eq_ignore_ascii_case("public domain") {
            Some(Self::PublicDomain)
        } else if notice
            .split_whitespace()
            .any(|w| w.eq_ignore_ascii_case("CC0"))
        {
            Some(Self::Cc0)
        } else {
            None
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::PublicDomain => "Public domain",
            Self::Cc0 => "CC0 1.0",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::PublicDomain => "public-domain",
            Self::Cc0 => "CC0-1.0",
        }
    }
    /// The dedication's text, where it has one.
    pub fn url(self) -> Option<&'static str> {
        match self {
            Self::PublicDomain => None,
            Self::Cc0 => Some("https://creativecommons.org/publicdomain/zero/1.0/"),
        }
    }
}

/// One reviewed profile, as `catalogue.toml` pins it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// `<source>/<slug>`: stable while the source's file is listed.
    pub id: String,
    pub source: String,
    /// The file's name in the source's directory.
    pub file: String,
    /// The name it is installed under: the file's, unless another catalogue
    /// profile has that name too.
    pub filename: String,
    /// The DCP's UniqueCameraModel and ProfileName.
    pub camera: String,
    pub name: String,
    /// The DCP's ProfileCopyright, verbatim.
    pub notice: String,
    pub terms: Terms,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcesFile {
    schema: u32,
    source: Vec<Source>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogueFile {
    schema: u32,
    #[serde(default)]
    profile: Vec<Profile>,
}

/// A catalogue profile with its source.
#[derive(Debug)]
pub struct Entry {
    pub profile: Profile,
    pub source: Arc<Source>,
}

#[derive(Debug, Default)]
pub struct Catalogue {
    entries: Vec<Entry>,
}

/// Whether a catalogue profile is in the user's library already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Available,
    /// Its exact file, here: installed, or imported by hand under its own name.
    Installed(PathBuf),
    /// A different file has its name where it would be installed.
    Conflict,
}

/// The catalogue the app ships. A unit test parses it, so a malformed or
/// unsafe entry cannot ship.
pub fn catalogue() -> &'static Catalogue {
    static PARSED: LazyLock<Catalogue> = LazyLock::new(|| {
        Catalogue::parse(SOURCES, CATALOGUE).expect("data/camera-profiles/catalogue.toml")
    });
    &PARSED
}

impl Catalogue {
    /// Reads and checks both files: unique ids, pinned GitHub sources, safe file
    /// names, sizes and digests, and terms the notice states.
    pub fn parse(sources: &str, catalogue: &str) -> Result<Self> {
        let sources: SourcesFile = toml::from_str(sources).context("sources.toml")?;
        let catalogue: CatalogueFile = toml::from_str(catalogue).context("catalogue.toml")?;
        ensure!(
            sources.schema == 1 && catalogue.schema == 1,
            "Unknown schema"
        );
        let mut checked: Vec<Arc<Source>> = Vec::new();
        for s in sources.source {
            ensure!(slug(&s.id), "Source id {:?}", s.id);
            ensure!(
                checked.iter().all(|c| c.id != s.id),
                "Source {} twice",
                s.id
            );
            ensure!(github(&s.repository).is_some(), "{}: repository", s.id);
            ensure!(
                hex(&s.revision, 40),
                "{}: revision must be a full commit",
                s.id
            );
            ensure!(relative(&s.directory), "{}: directory", s.id);
            ensure!(relative(&s.repository_license_file), "{}: license", s.id);
            ensure!(s.homepage.starts_with("https://"), "{}: homepage", s.id);
            ensure!(
                [&s.name, &s.credit, &s.repository_license]
                    .iter()
                    .all(|t| !t.trim().is_empty()),
                "{}: name, credit and license",
                s.id
            );
            checked.push(Arc::new(s));
        }
        let mut entries: Vec<Entry> = Vec::new();
        for p in catalogue.profile {
            let source = checked
                .iter()
                .find(|s| s.id == p.source)
                .with_context(|| format!("{}: unknown source", p.id))?
                .clone();
            ensure!(
                p.id.strip_prefix(&format!("{}/", p.source))
                    .is_some_and(slug),
                "Profile id {:?}",
                p.id
            );
            ensure!(dcp_name(&p.file), "{}: file", p.id);
            ensure!(dcp_name(&p.filename), "{}: filename", p.id);
            ensure!(
                (1..=MAX_BYTES).contains(&p.size) && hex(&p.sha256, 64),
                "{}: size and digest",
                p.id
            );
            ensure!(
                !p.camera.trim().is_empty() && !p.name.trim().is_empty(),
                "{}: camera and name",
                p.id
            );
            ensure!(
                Terms::of_notice(&p.notice) == Some(p.terms),
                "{}: its notice does not state these terms",
                p.id
            );
            for e in &entries {
                ensure!(e.profile.id != p.id, "{} twice", p.id);
                ensure!(
                    !e.profile.filename.eq_ignore_ascii_case(&p.filename),
                    "{} and {} install as the same file",
                    e.profile.id,
                    p.id
                );
                ensure!(
                    e.profile.sha256 != p.sha256,
                    "{} and {} are the same file",
                    e.profile.id,
                    p.id
                );
            }
            entries.push(Entry { profile: p, source });
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The profiles for a RAW photo's camera, under the rule the library lists
    /// profiles by, so an installed one is listed for the photo. Never another
    /// body's, and none for a rendered file.
    pub fn matching(&self, m: &Metadata) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.fits(m)).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.profile.id == id)
    }
}

impl Entry {
    pub fn fits(&self, m: &Metadata) -> bool {
        !m.is_rendered() && camera_fits(&self.profile.camera, m)
    }

    /// The pinned file's download.
    pub fn url(&self) -> String {
        let (owner, repo) = github(&self.source.repository).expect("checked on parse");
        format!(
            "https://raw.githubusercontent.com/{owner}/{repo}/{}/{}",
            self.source.revision,
            self.path()
        )
    }

    /// The pinned file's page on GitHub.
    pub fn page(&self) -> String {
        format!(
            "{}/blob/{}/{}",
            self.source.repository,
            self.source.revision,
            self.path()
        )
    }

    /// The repository's license at the pinned revision.
    pub fn repository_license_url(&self) -> String {
        format!(
            "{}/blob/{}/{}",
            self.source.repository,
            self.source.revision,
            encode_path(&self.source.repository_license_file)
        )
    }

    fn path(&self) -> String {
        encode_path(&format!("{}/{}", self.source.directory, self.profile.file))
    }

    /// Checks the transport's bounded bytes before parsing or publishing anything:
    /// the pinned size and digest, then that the DCP is the one reviewed (name,
    /// camera, notice), that it is supported and that it fits the photo.
    pub fn verify(&self, bytes: &[u8], m: &Metadata) -> Result<CameraProfile> {
        let p = &self.profile;
        ensure!(self.fits(m), "This profile is for another camera");
        ensure!(
            bytes.len() as u64 == p.size,
            "Profile download has the wrong size"
        );
        ensure!(
            checksum(bytes) == p.sha256,
            "Profile download failed checksum verification"
        );
        let profile = from_bytes(bytes)?;
        profile.ensure_camera(m)?;
        ensure!(
            profile.name == p.name && profile.camera == p.camera && profile.copyright == p.notice,
            "Downloaded profile is not the one reviewed"
        );
        Ok(profile)
    }

    /// Where the profile is in the user's library, if it is. Reads at most a few
    /// file headers and one file of the pinned size per library folder: call it
    /// when the photo's camera or the library changes, not every frame.
    pub fn status(&self) -> Status {
        self.status_in(
            &library::library_dirs(),
            &crate::storage::data_dir().join("camera-profiles"),
        )
    }

    fn status_in(&self, roots: &[PathBuf], destination: &Path) -> Status {
        let p = &self.profile;
        let names = [&p.filename, &p.file];
        for dir in std::iter::once(destination).chain(roots.iter().map(PathBuf::as_path)) {
            for name in names {
                let path = dir.join(name);
                if self.holds(&path) {
                    return Status::Installed(path);
                }
            }
        }
        if destination.join(&p.filename).symlink_metadata().is_ok() {
            Status::Conflict
        } else {
            Status::Available
        }
    }

    /// Whether `path` is this profile's exact file.
    pub fn holds(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == self.profile.size)
            && std::fs::read(path).is_ok_and(|b| checksum(&b) == self.profile.sha256)
    }

    /// The profile in the installed file at `path`, read once (at most the pinned
    /// size and a byte) and verified as a download is: what is used is the bytes
    /// checked, never the file read again. Fails for any other file there.
    pub fn read_installed(&self, path: &Path, m: &Metadata) -> Result<CameraProfile> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(self.profile.size + 1)
            .read_to_end(&mut bytes)?;
        self.verify(&bytes, m)
    }

    /// Installs verified bytes through the same validated, atomic, no-clobber
    /// import as a DCP the user chooses. The application captures `destination`
    /// before starting the job. Returns the installed file.
    pub fn install_into(&self, bytes: &[u8], m: &Metadata, destination: &Path) -> Result<PathBuf> {
        self.verify(bytes, m)?;
        let staging = tempfile::tempdir()?;
        let source = staging.path().join(&self.profile.filename);
        std::fs::write(&source, bytes)?;
        let imported = library::import_into(&[source], destination, LutSpace::Srgb)?;
        // Another process can import a file during the importer's scan. Never
        // report that competing file as this verified download's success.
        let installed = destination.join(&self.profile.filename);
        ensure!(
            self.holds(&installed),
            "Installed profile does not match the verified download"
        );
        Ok(imported
            .into_iter()
            .next()
            .map_or(installed, |imported| imported.path))
    }
}

fn checksum(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// One path component a library can hold: no separators, drive letters,
/// control characters, or hidden and relative names.
fn component(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && !s.ends_with(' ')
        && !s.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
}

fn dcp_name(s: &str) -> bool {
    component(s)
        && Path::new(s)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
}

fn relative(s: &str) -> bool {
    s.split('/').all(component)
}

/// The owner and repository of `https://github.com/<owner>/<repo>`.
fn github(url: &str) -> Option<(&str, &str)> {
    let (owner, repo) = url.strip_prefix("https://github.com/")?.split_once('/')?;
    let name = |s: &str| {
        component(s)
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    };
    (name(owner) && name(repo)).then_some((owner, repo))
}

/// `path` with each component percent-encoded for a URL.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|part| {
            part.bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{b:02X}"),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// A DCP in a checkout of a source's pinned revision, for [`generate`].
pub struct Found {
    pub source: String,
    pub file: String,
    pub bytes: Vec<u8>,
}

/// A file [`generate`] left out, and why.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Excluded {
    pub source: String,
    pub file: String,
    pub reason: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Generated {
    pub profiles: Vec<Profile>,
    pub excluded: Vec<Excluded>,
}

/// The catalogue of every DCP found in `sources`' checkouts that the importer
/// supports and whose own notice dedicates it; the rest are excluded with a
/// reason. The result depends only on the files: sources in the order given,
/// files by name, the first source's copy of identical bytes, and a file name
/// two profiles share installed with its source's name added.
pub fn generate(sources: &[Source], mut found: Vec<Found>) -> Result<Generated> {
    let order = |id: &str| sources.iter().position(|s| s.id == id);
    for f in &found {
        ensure!(order(&f.source).is_some(), "{}: unknown source", f.source);
    }
    found.sort_by(|a, b| (order(&a.source), &a.file).cmp(&(order(&b.source), &b.file)));
    let mut profiles: Vec<Profile> = Vec::new();
    let mut excluded = Vec::new();
    for f in found {
        let mut exclude = |reason: String| {
            excluded.push(Excluded {
                source: f.source.clone(),
                file: f.file.clone(),
                reason,
            })
        };
        if !dcp_name(&f.file) {
            exclude("its file name cannot be installed".into());
            continue;
        }
        if f.bytes.len() as u64 > MAX_BYTES {
            exclude("larger than 16 MB".into());
            continue;
        }
        let dcp = match from_bytes(&f.bytes) {
            Ok(dcp) => dcp,
            Err(e) => {
                exclude(format!("not supported: {e:#}"));
                continue;
            }
        };
        let Some(terms) = Terms::of_notice(&dcp.copyright) else {
            exclude(if dcp.copyright.trim().is_empty() {
                "no copyright notice: its terms are unknown".into()
            } else {
                format!(
                    "its notice ({:?}) grants no terms beyond copyright",
                    dcp.copyright
                )
            });
            continue;
        };
        if dcp.camera.trim().is_empty() || dcp.name.trim().is_empty() {
            exclude("no camera or profile name".into());
            continue;
        }
        let sha256 = checksum(&f.bytes);
        if let Some(same) = profiles.iter().find(|p| p.sha256 == sha256) {
            exclude(format!("the same file as {}", same.id));
            continue;
        }
        let stem = Path::new(&f.file)
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let base = format!("{}/{}", f.source, slugify(&stem));
        let mut id = base.clone();
        let mut n = 2;
        while profiles.iter().any(|p| p.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        profiles.push(Profile {
            id,
            source: f.source.clone(),
            filename: f.file.clone(),
            file: f.file,
            camera: dcp.camera,
            name: dcp.name,
            notice: dcp.copyright,
            terms,
            size: f.bytes.len() as u64,
            sha256,
        });
    }
    let names: Vec<String> = profiles
        .iter()
        .map(|p| p.file.to_ascii_lowercase())
        .collect();
    for (i, p) in profiles.iter_mut().enumerate() {
        if names.iter().filter(|n| **n == names[i]).count() > 1 {
            let source = &sources[order(&p.source).expect("checked")];
            let stem = Path::new(&p.file).file_stem().unwrap_or_default();
            p.filename = format!("{} ({}).dcp", stem.to_string_lossy(), source.name);
        }
    }
    Ok(Generated { profiles, excluded })
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "profile".into()
    } else {
        out
    }
}

impl Generated {
    /// `catalogue.toml`, as checked in.
    pub fn catalogue_toml(&self) -> String {
        let mut out = String::from(HEADER);
        out.push_str("schema = 1\n");
        for p in &self.profiles {
            out.push_str("\n[[profile]]\n");
            for (key, value) in [
                ("id", &p.id),
                ("source", &p.source),
                ("file", &p.file),
                ("filename", &p.filename),
                ("camera", &p.camera),
                ("name", &p.name),
                ("notice", &p.notice),
            ] {
                out.push_str(&format!("{key} = {}\n", quote(value)));
            }
            out.push_str(&format!("terms = {}\n", quote(p.terms.key())));
            out.push_str(&format!("size = {}\n", p.size));
            out.push_str(&format!("sha256 = {}\n", quote(&p.sha256)));
        }
        out
    }

    /// `excluded.toml`, as checked in.
    pub fn excluded_toml(&self) -> String {
        let mut out = String::from(EXCLUDED_HEADER);
        out.push_str("schema = 1\n");
        for e in &self.excluded {
            out.push_str("\n[[excluded]]\n");
            for (key, value) in [
                ("source", &e.source),
                ("file", &e.file),
                ("reason", &e.reason),
            ] {
                out.push_str(&format!("{key} = {}\n", quote(value)));
            }
        }
        out
    }
}

/// The files `excluded.toml` lists.
pub fn parse_excluded(text: &str) -> Result<Vec<Excluded>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        schema: u32,
        #[serde(default)]
        excluded: Vec<Excluded>,
    }
    let file: File = toml::from_str(text)?;
    ensure!(file.schema == 1, "Unknown schema");
    Ok(file.excluded)
}

/// The sources `sources.toml` describes.
pub fn parse_sources(text: &str) -> Result<Vec<Source>> {
    let file: SourcesFile = toml::from_str(text)?;
    ensure!(file.schema == 1, "Unknown schema");
    Ok(file.source)
}

const HEADER: &str = "\
# The camera-profile catalogue: generated from checkouts of the revisions in
# sources.toml, never edited by hand (docs/lightroom-profiles.md#camera-profile-catalogue):
#
#     cargo run -p rawmakase-model --example profile_catalogue -- generate \\
#         rawtherapee=<checkout>/rtdata/dcpprofiles art=<checkout>/data/dcpprofiles
#
# terms come from each DCP's own notice (its ProfileCopyright, verbatim here).
";

const EXCLUDED_HEADER: &str = "\
# The DCPs in sources.toml's checkouts that catalogue.toml leaves out, and why.
# Generated with catalogue.toml; never edited by hand.
";

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod tests;
