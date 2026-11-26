//! Managing the profile library: listing every file in it, whatever camera it is
//! for, renaming a look, moving looks to another group of the Profile menu, and
//! removing files (docs/lightroom-profiles.md, *Manage the library*).
//!
//! Only the user's library folder (`data_dir()/camera-profiles`, the one imports
//! write to) is managed. A look keeps its UUID, tables and settings through a rename
//! or a move: only its `crs:Name` or `crs:Group` is rewritten. Edits are not
//! touched, as each embeds the profile it uses; presets and sidecars that name a
//! renamed look still find it by its UUID (`xmp::Preset::resolve_look` in interop).
use super::{
    enhanced::{LookBase, LookFile, with_label},
    from_bytes,
};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

/// What a library file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A DCP camera profile.
    Camera,
    /// An XMP look profile, an imported LUT's included.
    Look,
    /// A file RAWmakase can't read as either.
    Unreadable,
}

/// One file in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub file: PathBuf,
    pub kind: Kind,
    /// The profile's name; the file's name for an unreadable one.
    pub name: String,
    /// A look's UUID; empty otherwise.
    pub uuid: String,
    /// A look's `crs:Group`, the Profile menu's submenu it lists under.
    pub group: String,
    /// A DCP's camera, or the camera a look is restricted to (empty for any).
    pub camera: String,
    /// The base profile a camera look names (Adobe Standard for Adobe Color).
    pub base: Option<String>,
    /// A look of Adobe's (Adobe Color, the Artistic looks): Lightroom edits and
    /// RAWmakase's fallbacks name these, so they are not renamed or moved.
    pub adobe: bool,
    /// Why the file can't be read, for an unreadable one.
    pub error: Option<String>,
}
impl Entry {
    /// Whether the look's name and group may be changed here.
    pub fn editable(&self) -> bool {
        self.kind == Kind::Look && !self.adobe
    }
    /// The key that favourites and hidden state keep for this profile: a look's
    /// UUID, which a rename keeps, else its name.
    pub fn key(&self) -> String {
        key(
            &self.name,
            (!self.uuid.is_empty()).then_some(self.uuid.as_str()),
        )
    }
}
/// The key [`Entry::key`] gives a profile named `name` with the look UUID `uuid`.
pub fn key(name: &str, uuid: Option<&str>) -> String {
    match uuid {
        Some(uuid) if !uuid.is_empty() => format!("look:{}", uuid.to_ascii_uppercase()),
        _ => format!("profile:{name}"),
    }
}

/// The names no imported profile may take: RAWmakase's own, and Adobe Standard and
/// Adobe Color, which Lightroom edits fall back from by name.
fn reserved(name: &str) -> bool {
    [
        super::open::STANDARD,
        super::open::COLOR,
        super::fitted::LOOK,
        super::fitted::MATRIX_BASE,
        "Adobe Standard",
        "Adobe Color",
    ]
    .contains(&name)
}

/// A profile library folder: the one imports go to
/// ([`super::ProfileLibrary::manager`]) or a test's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    dir: PathBuf,
}
impl Library {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    /// Every file in the library, by name, then file. Hidden files and the staging
    /// files of a write in progress are left out.
    pub fn entries(&self) -> Vec<Entry> {
        let mut files = Vec::new();
        super::library::collect(&self.dir, 0, &mut files);
        let mut entries: Vec<Entry> = files
            .into_iter()
            .filter(|f| {
                !crate::storage::is_hidden(f)
                    && !f
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("partial"))
            })
            .map(|file| read(&file).unwrap_or_else(|e| unreadable(file, e)))
            .collect();
        entries.sort_by(|a, b| (&a.name, &a.file).cmp(&(&b.name, &b.file)));
        entries
    }
    /// `file`, checked to be a file in this library.
    fn member(&self, file: &Path) -> Result<PathBuf> {
        let dir = std::fs::canonicalize(&self.dir).context("The profile library is missing")?;
        let found = std::fs::canonicalize(file)
            .with_context(|| format!("{} is no longer in the library", display(file)))?;
        ensure!(
            found.starts_with(&dir) && found != dir && found.is_file(),
            "{} is not in the profile library",
            display(file)
        );
        Ok(found)
    }
    /// The look file `file`, which may be renamed and moved here, with its text.
    fn editable(&self, file: &Path) -> Result<(PathBuf, String)> {
        let file = self.member(file)?;
        let entry = read(&file)?;
        ensure!(
            entry.kind == Kind::Look,
            "Only looks can be renamed or moved: {} is a camera profile",
            entry.name
        );
        ensure!(
            !entry.adobe,
            "{} is one of Adobe's looks, which keep their names and groups",
            entry.name
        );
        let text = std::fs::read_to_string(&file)?;
        Ok((file, text))
    }
    /// Renames the look in `file` to `name`, keeping its UUID, group and tables. A
    /// name another profile in the library has, or one of RAWmakase's or Adobe
    /// Standard's and Color's, is refused: names identify looks in the Profile
    /// menu, presets and sidecars.
    pub fn rename_look(&self, file: &Path, name: &str) -> Result<()> {
        let name = label(name, "name")?;
        ensure!(!name.is_empty(), "A profile needs a name");
        let (file, text) = self.editable(file)?;
        let before = LookFile::summary(&text)?;
        if before.name == name {
            return Ok(());
        }
        ensure!(
            !reserved(&name),
            "{name} is a name RAWmakase keeps for its own profile"
        );
        let entries = self.entries();
        if let Some(other) = entries
            .iter()
            .find(|e| e.kind != Kind::Unreadable && e.name == name && !same(&e.file, &file))
        {
            bail!(
                "Another profile is named {name} ({})",
                other.file.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        let renamed = with_label(&text, "Name", &name)?;
        let after = LookFile::summary(&renamed)?;
        ensure!(
            after.name == name && after.uuid == before.uuid && after.group == before.group,
            "This look can't be renamed to {name}"
        );
        LookFile::parse(&renamed).context("This look can't be renamed")?;
        write(&file, &renamed)
    }
    /// Moves the looks in `files` to the Profile menu's group `group`, or to the top
    /// level for an empty one; returns how many changed. Every file is checked and
    /// rewritten in memory before any is written; a write that still fails is a
    /// [`Partial`] error, saying how many were moved before it.
    pub fn set_group(&self, files: &[PathBuf], group: &str) -> Result<usize> {
        let group = label(group, "group")?;
        let mut changes = Vec::new();
        for file in files {
            let (file, text) = self.editable(file)?;
            let before = LookFile::summary(&text)?;
            if before.group == group {
                continue;
            }
            let moved = with_label(&text, "Group", &group)?;
            let after = LookFile::summary(&moved)?;
            ensure!(
                after.group == group && after.name == before.name && after.uuid == before.uuid,
                "{} can't be moved to {group}",
                before.name
            );
            LookFile::parse(&moved).with_context(|| format!("{} can't be moved", before.name))?;
            changes.push((file, moved));
        }
        // A write that fails leaves the looks before it moved: say how many.
        for (done, (file, text)) in changes.iter().enumerate() {
            write(file, text).map_err(|e| Partial {
                done,
                error: format!("{}: {e:#}", display(file)),
            })?;
        }
        Ok(changes.len())
    }
    /// Removes `files` from the library through `trash`, which moves a file to the
    /// system's Trash: nothing is deleted outright. Each file is checked to be in the
    /// library first; a failure is reported and the rest still go.
    pub fn remove(&self, files: &[PathBuf], mut trash: impl FnMut(&Path) -> Result<()>) -> Removal {
        let mut removal = Removal::default();
        for file in files {
            match self.member(file).and_then(|found| trash(&found)) {
                Ok(()) => removal.removed += 1,
                Err(e) => removal.failed.push((file.clone(), format!("{e:#}"))),
            }
        }
        removal
    }
}
/// A batch [`Library::set_group`] stopped partway: `done` looks were moved before
/// a write failed.
#[derive(Debug)]
pub struct Partial {
    pub done: usize,
    pub error: String,
}
impl std::fmt::Display for Partial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} moved before {}", self.done, self.error)
    }
}
impl std::error::Error for Partial {}
/// What [`Library::remove`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    pub removed: usize,
    pub failed: Vec<(PathBuf, String)>,
}

/// The looks among `entries` that removing `files` leaves without the base profile
/// they name: camera looks such as Adobe Color go over a DCP of that name, and stop
/// loading when no DCP of the name for their camera is left. Creative looks name
/// none and go over another profile instead, so they are never listed.
pub fn dependents(entries: &[Entry], files: &[PathBuf]) -> Vec<String> {
    let removed = |e: &Entry| files.iter().any(|f| same(f, &e.file));
    let fits = |look: &Entry, dcp: &Entry| {
        look.camera.is_empty() || look.camera.eq_ignore_ascii_case(&dcp.camera)
    };
    let mut names: Vec<String> = entries
        .iter()
        .filter(|look| look.kind == Kind::Look && !removed(look))
        .filter(|look| {
            let Some(base) = &look.base else {
                return false;
            };
            let bases = entries
                .iter()
                .filter(|e| e.kind == Kind::Camera && e.name == *base && fits(look, e));
            let mut gone = false;
            for dcp in bases {
                if !removed(dcp) {
                    return false;
                }
                gone = true;
            }
            gone
        })
        .map(|look| look.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// A label typed by the user, trimmed; control characters are refused, as XML
/// drops them and the label would read back otherwise.
fn label(text: &str, what: &str) -> Result<String> {
    let text = text.trim();
    ensure!(
        !text.chars().any(char::is_control),
        "A profile {what} can't hold control characters"
    );
    ensure!(text.chars().count() <= 256, "That {what} is too long");
    Ok(text.to_string())
}
fn read(file: &Path) -> Result<Entry> {
    ensure!(
        std::fs::metadata(file)?.len() <= 16_000_000,
        "Profile too large"
    );
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("dcp") => {
            let profile = from_bytes(&std::fs::read(file)?)?;
            Ok(Entry {
                file: file.into(),
                kind: Kind::Camera,
                name: profile.name,
                uuid: String::new(),
                group: String::new(),
                camera: profile.camera,
                base: None,
                adobe: false,
                error: None,
            })
        }
        Some("xmp") => {
            let look = LookFile::summary(&std::fs::read_to_string(file)?)?;
            Ok(Entry {
                file: file.into(),
                kind: Kind::Look,
                name: look.name,
                uuid: look.uuid,
                group: look.group,
                camera: look.restriction,
                base: match look.base {
                    LookBase::Named(name) => Some(name),
                    LookBase::Any => None,
                },
                adobe: look.adobe,
                error: None,
            })
        }
        _ => bail!("Not a DCP or XMP profile"),
    }
}
fn unreadable(file: PathBuf, e: anyhow::Error) -> Entry {
    Entry {
        name: file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        file,
        kind: Kind::Unreadable,
        uuid: String::new(),
        group: String::new(),
        camera: String::new(),
        base: None,
        adobe: false,
        error: Some(format!("{e:#}")),
    }
}
fn write(file: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    crate::storage::write_atomic(file, crate::storage::Replace::Overwrite, |f| {
        Ok(f.write_all(text.as_bytes())?)
    })
}
/// Whether two paths are the same file, as the file system resolves them.
fn same(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .is_some_and(|a| std::fs::canonicalize(b).ok() == Some(a))
}
fn display(file: &Path) -> String {
    file.file_name()
        .unwrap_or(file.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
#[path = "manage_tests.rs"]
mod tests;
