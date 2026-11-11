//! `<data dir>/keymap.toml`, the user's key map, and the one owner of it.
//!
//! ```toml
//! version = 1
//!
//! [bindings]
//! "crop" = ["R"]
//! "loupe" = ["E", "Return"]
//! "compare" = []           # no key
//! ```
//!
//! Only the commands the user changed appear, each with its whole list of
//! chords; every other command keeps this release's defaults. The file is
//! edited in place (`toml_edit`), so comments, order, keys this release doesn't
//! know and commands it doesn't know (a newer release's, an extension's) stay
//! as they are. Older releases don't read it at all.
//!
//! A file that doesn't read (bad TOML, the wrong shape, a newer `version`) is
//! never written: the defaults run, and Preferences > Shortcuts only offers
//! Reset All, which moves the file aside first. One command's entry that
//! doesn't read skips that command alone; it is kept as written until the user
//! gives the command other keys.
//!
//! Each change is made to the file as it is when the change is saved, read
//! again first, so a second RAWmakase's changes, or entries a newer release
//! or an extension added since start-up, are kept. A file that no longer
//! reads then (or that a newer release wrote) is left as it is, as at
//! start-up.
//!
//! Memory never runs chords the file doesn't hold: after a failed write the
//! file is read back, because [`crate::storage::write_atomic`] can fail after
//! the new file is in place (syncing the folder).
use super::rules::{self, Os};
use super::{Chord, Effective};
use crate::app::shell::commands::registry::Dynamic;
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use toml_edit::{DocumentMut, Item, Value};

/// The `version` this release writes, and the newest it reads.
pub(in crate::app) const VERSION: i64 = 1;

/// How the key map file is read and written, so tests can make it fail.
pub(in crate::app) trait Disk: Send {
    fn read(&self, path: &Path) -> io::Result<String>;
    /// Replaces `path` with `text`, whole or not at all.
    fn write(&self, path: &Path, text: &str) -> anyhow::Result<()>;
    /// Renames `from` to `to`, failing if `to` exists.
    fn move_aside(&self, from: &Path, to: &Path) -> io::Result<()>;
}

/// The file system.
struct Files;

impl Disk for Files {
    fn read(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn write(&self, path: &Path, text: &str) -> anyhow::Result<()> {
        use std::io::Write;
        // The data folder may not exist before the first file in it.
        std::fs::create_dir_all(crate::storage::parent_dir(path))?;
        crate::storage::write_atomic(path, crate::storage::Replace::Overwrite, |f| {
            Ok(f.write_all(text.as_bytes())?)
        })
    }
    fn move_aside(&self, from: &Path, to: &Path) -> io::Result<()> {
        crate::storage::rename_noclobber(from, to)
    }
}

/// Why the file isn't used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Unreadable {
    /// Not TOML, not shaped as a key map, or not readable at all.
    Malformed(String),
    /// Written by a newer release.
    Newer(i64),
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "{why}"),
            Self::Newer(v) => write!(f, "it is version {v}, from a newer RAWmakase"),
        }
    }
}

/// What the file is to this release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum State {
    /// No file: the defaults, and the first change writes one.
    Absent,
    Read,
    /// The defaults run, and nothing writes the file but Reset All.
    Unreadable(Unreadable),
}

/// An entry of the file this release skips, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Skipped {
    pub id: String,
    pub why: String,
}

/// How a change was saved.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) enum Saved {
    Durably,
    /// The file holds the change, but the write reported an error after
    /// putting it in place, so it may not survive a crash.
    MaybeNotDurably(String),
}

/// The user's key map: the file's bindings in effect, and every change made
/// to them.
pub(in crate::app) struct KeymapFile {
    /// None in a test editor: changes stay in memory and nothing is read.
    path: Option<PathBuf>,
    disk: Box<dyn Disk>,
    state: State,
    /// The file as read, edited and written back.
    doc: DocumentMut,
    effective: Arc<Effective>,
    /// Entries for commands this release knows, skipped.
    skipped: Vec<Skipped>,
    /// Entries for commands this release doesn't know, kept as they are:
    /// an extension's command while its extension is off or not installed
    /// among them, so its keys come back with it.
    unknown: Vec<String>,
    /// The enabled extensions' commands, which the file's entries may name.
    dynamic: Vec<&'static Dynamic>,
}

/// What a file's text holds.
struct Parsed {
    doc: DocumentMut,
    user: BTreeMap<&'static str, Vec<Chord>>,
    skipped: Vec<Skipped>,
    unknown: Vec<String>,
}

impl KeymapFile {
    /// The defaults and no file: a test editor's.
    pub(in crate::app) fn defaults() -> Self {
        Self {
            path: None,
            disk: Box::new(Files),
            state: State::Absent,
            doc: fresh(),
            effective: Arc::default(),
            skipped: Vec::new(),
            unknown: Vec::new(),
            dynamic: Vec::new(),
        }
    }
    /// The key map in `path`.
    pub(in crate::app) fn load(path: PathBuf) -> Self {
        Self::load_with(path, Box::new(Files))
    }
    /// The key map in `path`, read and written through `disk`.
    pub(in crate::app) fn load_with(path: PathBuf, disk: Box<dyn Disk>) -> Self {
        let mut file = Self {
            path: Some(path),
            disk,
            ..Self::defaults()
        };
        file.reread();
        file
    }
    /// The file in the data folder.
    pub(in crate::app) fn path_in_data_dir() -> PathBuf {
        crate::storage::data_dir().join("keymap.toml")
    }

    pub(in crate::app) fn effective(&self) -> Arc<Effective> {
        self.effective.clone()
    }
    /// Makes `dynamic`, the enabled extensions' commands, the extension
    /// commands in effect, and reads the key map's entries for them again
    /// from the file as last read: an entry for a command whose extension
    /// was just turned on takes effect, and one whose extension was turned
    /// off stays in the file, unknown, until it comes back.
    pub(in crate::app) fn set_dynamic(&mut self, dynamic: Vec<&'static Dynamic>) {
        if dynamic == self.dynamic {
            return;
        }
        self.dynamic = dynamic;
        match &self.state {
            State::Unreadable(why) => self.unreadable(why.clone()),
            state => {
                let state = state.clone();
                match parse(&self.doc.to_string(), &self.dynamic) {
                    Ok(parsed) => self.adopt(state, parsed),
                    Err(why) => self.unreadable(why),
                }
            }
        }
    }
    pub(in crate::app) fn state(&self) -> &State {
        &self.state
    }
    /// Whether changes can be made: not while the file doesn't read.
    pub(in crate::app) fn editable(&self) -> bool {
        !matches!(self.state, State::Unreadable(_))
    }
    pub(in crate::app) fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }
    /// The ids of the commands the file has but this release doesn't know.
    pub(in crate::app) fn unknown(&self) -> &[String] {
        &self.unknown
    }
    pub(in crate::app) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// What to say at start-up about the file, if anything.
    pub(in crate::app) fn report(&self) -> Option<String> {
        match &self.state {
            State::Unreadable(why) => Some(format!(
                "keymap.toml isn't used: {why}. The default shortcuts apply; \
                 Preferences > Shortcuts can reset it."
            )),
            _ if !self.skipped.is_empty() => {
                let first = &self.skipped[0];
                let more = match self.skipped.len() - 1 {
                    0 => String::new(),
                    1 => " and one other".into(),
                    n => format!(" and {n} others"),
                };
                Some(format!(
                    "keymap.toml: the keys for {}{more} were skipped ({}); \
                     their default shortcuts apply.",
                    first.id, first.why
                ))
            }
            _ => None,
        }
    }

    /// Gives `id` exactly `chords` (none leaves it with no key), and saves
    /// the file. A refused chord, a file that doesn't read or a write that
    /// fails changes nothing and says why.
    pub(in crate::app) fn set(&mut self, id: &str, chords: &[Chord]) -> Result<Saved, String> {
        let command = self
            .effective
            .command(id)
            .ok_or_else(|| format!("no command {id}"))?;
        // Before the chords: an empty list would take a fixed command's keys.
        if !command.rebindable {
            return Err(format!("{}: {}", command.title, rules::Refusal::Fixed));
        }
        let bindings = self.effective.bindings_of(id);
        for chord in chords {
            if let Some(why) = rules::refusal(command, &bindings, *chord, Os::CURRENT) {
                return Err(format!("{chord} can't be used: {why}"));
            }
        }
        let mut list = toml_edit::Array::new();
        let mut seen = Vec::new();
        for chord in chords {
            if !seen.contains(chord) {
                seen.push(*chord);
                list.push(chord.to_string());
            }
        }
        self.change(|doc| {
            let table = bindings_table(doc);
            // In place, so the comment before the entry and after it stay.
            match table.get_mut(id).and_then(Item::as_value_mut) {
                Some(value) => {
                    let decor = value.decor().clone();
                    *value = Value::Array(list);
                    *value.decor_mut() = decor;
                }
                None => {
                    table.insert(id, Item::Value(Value::Array(list)));
                }
            }
        })
    }
    /// Gives `id` its default keys again.
    pub(in crate::app) fn reset(&mut self, id: &str) -> Result<Saved, String> {
        self.change(|doc| {
            bindings_table(doc).remove(id);
        })
    }
    /// Every command back to its default keys. The file's other content
    /// stays; a file that doesn't read is moved aside, to
    /// `keymap.toml.broken`, and a fresh one written.
    pub(in crate::app) fn reset_all(&mut self) -> Result<Saved, String> {
        if let (State::Unreadable(_), Some(path)) = (&self.state, &self.path) {
            let aside = self.move_aside(&path.clone())?;
            self.state = State::Absent;
            self.doc = fresh();
            return self
                .change(|_| {})
                .map_err(|e| format!("{e} The old file is kept as {}.", aside.display()));
        }
        self.change(|doc| bindings_table(doc).clear())
    }

    /// Moves the unreadable file to a free `keymap.toml.broken` name.
    fn move_aside(&self, path: &Path) -> Result<PathBuf, String> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "keymap.toml".into());
        let mut last = String::new();
        for n in 1..=20 {
            let suffix = if n == 1 {
                ".broken".to_string()
            } else {
                format!(".broken-{n}")
            };
            let aside = path.with_file_name(format!("{name}{suffix}"));
            match self.disk.move_aside(path, &aside) {
                Ok(()) => return Ok(aside),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    last = e.to_string();
                    break;
                }
            }
        }
        Err(format!(
            "keymap.toml could not be moved aside, so it was left as it is: {last}"
        ))
    }

    /// Edits the file with `edit` and writes it. In memory alone without a
    /// file. The edit applies to the file as it is now, read again first:
    /// another RAWmakase may have changed it, or a newer release written it,
    /// since it was read. A file that no longer reads is left as it is, and
    /// the key map becomes read-only. After a failed write, whatever the file
    /// then holds is in effect.
    fn change(&mut self, edit: impl FnOnce(&mut DocumentMut)) -> Result<Saved, String> {
        if let State::Unreadable(why) = &self.state {
            return Err(format!(
                "keymap.toml isn't used ({why}), so it is left as it is. Reset All replaces it."
            ));
        }
        let mut doc = match &self.path {
            None => self.doc.clone(),
            Some(_) => {
                self.reread();
                if let State::Unreadable(why) = &self.state {
                    return Err(format!(
                        "keymap.toml changed since it was read and isn't used now ({why}), \
                         so it is left as it is. The default shortcuts apply."
                    ));
                }
                self.doc.clone()
            }
        };
        edit(&mut doc);
        let text = doc.to_string();
        let parsed = parse(&text, &self.dynamic)
            .map_err(|e| format!("The key map would not read back: {e}"))?;
        let Some(path) = self.path.clone() else {
            self.adopt(State::Read, parsed);
            return Ok(Saved::Durably);
        };
        match self.disk.write(&path, &text) {
            Ok(()) => {
                self.adopt(State::Read, parsed);
                Ok(Saved::Durably)
            }
            Err(error) => {
                let held = self.reread().is_some_and(|read| read == text);
                if held {
                    Ok(Saved::MaybeNotDurably(format!("{error:#}")))
                } else {
                    Err(format!(
                        "The shortcut was not saved: {error:#}. The keys in keymap.toml apply."
                    ))
                }
            }
        }
    }

    /// Reads the file again and puts what it holds in effect: its text, when
    /// it reads.
    fn reread(&mut self) -> Option<String> {
        let path = self.path.as_ref()?;
        match self.disk.read(path) {
            Ok(text) => match parse(&text, &self.dynamic) {
                Ok(parsed) => {
                    self.adopt(State::Read, parsed);
                    Some(text)
                }
                Err(why) => {
                    self.unreadable(why);
                    None
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.adopt(
                    State::Absent,
                    Parsed {
                        doc: fresh(),
                        user: BTreeMap::new(),
                        skipped: Vec::new(),
                        unknown: Vec::new(),
                    },
                );
                None
            }
            Err(e) => {
                self.unreadable(Unreadable::Malformed(format!("it could not be read: {e}")));
                None
            }
        }
    }
    fn adopt(&mut self, state: State, parsed: Parsed) {
        self.state = state;
        self.doc = parsed.doc;
        self.effective = Arc::new(Effective::new(parsed.user).with_dynamic(self.dynamic.clone()));
        self.skipped = parsed.skipped;
        self.unknown = parsed.unknown;
    }
    fn unreadable(&mut self, why: Unreadable) {
        self.state = State::Unreadable(why);
        self.doc = fresh();
        self.effective = Arc::new(Effective::default().with_dynamic(self.dynamic.clone()));
        self.skipped.clear();
        self.unknown.clear();
    }
}

/// A new file's text: what it is, and the version.
fn fresh() -> DocumentMut {
    let mut doc = DocumentMut::new();
    doc.decor_mut().set_prefix(
        "# RAWmakase's keyboard shortcuts: the commands given other keys than their\n\
         # defaults, each with its whole list. Preferences > Shortcuts edits it.\n",
    );
    doc.insert("version", toml_edit::value(VERSION));
    let mut table = toml_edit::Table::new();
    table.decor_mut().set_prefix("\n");
    doc.insert("bindings", Item::Table(table));
    doc
}

/// The document's `[bindings]`, made if it has none.
fn bindings_table(doc: &mut DocumentMut) -> &mut dyn toml_edit::TableLike {
    if !doc.contains_key("bindings") {
        doc.insert("bindings", Item::Table(toml_edit::Table::new()));
    }
    doc["bindings"]
        .as_table_like_mut()
        .expect("parse only lets a table through")
}

/// What `text` holds, or why it isn't a key map this release reads.
fn parse(text: &str, dynamic: &[&'static Dynamic]) -> Result<Parsed, Unreadable> {
    let known = Effective::default().with_dynamic(dynamic.to_vec());
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        Unreadable::Malformed(format!("it isn't TOML: {}", e.message()))
    })?;
    match doc.get("version") {
        // A hand-written file may leave it out.
        None => {}
        Some(item) => match item.as_integer() {
            Some(v) if v > VERSION => return Err(Unreadable::Newer(v)),
            Some(v) if v >= 1 => {}
            _ => {
                return Err(Unreadable::Malformed(
                    "its version isn't a version number".into(),
                ));
            }
        },
    }
    let mut parsed = Parsed {
        doc: doc.clone(),
        user: BTreeMap::new(),
        skipped: Vec::new(),
        unknown: Vec::new(),
    };
    let Some(bindings) = doc.get("bindings") else {
        return Ok(parsed);
    };
    let bindings = bindings
        .as_table_like()
        .ok_or_else(|| Unreadable::Malformed("its bindings aren't a table".into()))?;
    for (id, item) in bindings.iter() {
        let Some(command) = known.command(id) else {
            parsed.unknown.push(id.to_string());
            continue;
        };
        match chords(command, item, &known) {
            Ok(chords) => {
                parsed.user.insert(command.id, chords);
            }
            Err(why) => parsed.skipped.push(Skipped {
                id: id.to_string(),
                why,
            }),
        }
    }
    Ok(parsed)
}

/// The chords of one command's entry, or why it is skipped.
fn chords(
    command: &'static crate::app::shell::commands::registry::Command,
    item: &Item,
    known: &Effective,
) -> Result<Vec<Chord>, String> {
    // Before the list: an empty one would take a fixed command's keys too.
    if !command.rebindable {
        return Err(rules::Refusal::Fixed.to_string());
    }
    let array = item
        .as_array()
        .ok_or_else(|| "not a list of keys".to_string())?;
    let bindings = known.bindings_of(command.id);
    let mut out: Vec<Chord> = Vec::new();
    for value in array.iter() {
        let text = value
            .as_str()
            .ok_or_else(|| format!("{} isn't a key in quotes", value.to_string().trim()))?;
        let chord = Chord::parse(text).map_err(|e| format!("\"{text}\": {e}"))?;
        if let Some(why) = rules::refusal(command, &bindings, chord, Os::CURRENT) {
            return Err(format!("{chord}: {why}"));
        }
        if !out.contains(&chord) {
            out.push(chord);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
