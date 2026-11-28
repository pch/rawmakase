//! Lightroom's Keyword Sets: named groups of nine keywords, kept as
//! `.lrtemplate` files outside any catalog, so a set works in every catalog.
//! RAWmakase saves its own in its data folder, in the form Lightroom writes
//! them, and reads Lightroom Classic's too (`Keyword Sets` in Lightroom's
//! application data), which it never changes: saving over one of Lightroom's
//! sets saves a copy of RAWmakase's own of that name. Lightroom's three
//! built-in sets come with RAWmakase.
use crate::lua;
use crate::storage::Replace;
use crate::xmp::local::Node;
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

/// The keywords a set holds.
pub const SLOTS: usize = 9;

/// Where a set was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// RAWmakase's own: renamed, changed and deleted in place.
    Own,
    /// Lightroom Classic's, read only.
    Lightroom,
    /// One of Lightroom's built-in sets, as RAWmakase ships them.
    BuiltIn,
}

/// A keyword set: its name and the keyword typed in each slot, empty where
/// none is. A slot is typed as in the Keywording panel (`Child < Parent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeywordSet {
    pub name: String,
    pub keywords: [String; SLOTS],
    pub origin: Origin,
}

/// Lightroom's built-in sets, as Lightroom Classic 15 ships them.
const BUILT_IN: [(&str, [&str; SLOTS]); 3] = [
    (
        "Outdoor Photography",
        [
            "Fall",
            "Winter",
            "People",
            "Spring",
            "Summer",
            "Wildlife",
            "Landscape",
            "Macro",
            "Flowers & Plants",
        ],
    ),
    (
        "Portrait Photography",
        [
            "Kids",
            "Babies",
            "Families",
            "Pets",
            "Headshots",
            "Individuals",
            "Outdoor",
            "Candid",
            "Fashion",
        ],
    ),
    (
        "Wedding Photography",
        [
            "Pre-Ceremony",
            "Ceremony",
            "Reception",
            "Wedding Party",
            "Family",
            "Black & White",
            "Bride",
            "Groom",
            "Candid",
        ],
    ),
];

/// Reads a keyword set file: its title (else its internal name, else the
/// file's) and the nine `shortcutNtitle` slots, missing ones empty.
pub fn parse(text: &str, file_stem: &str) -> Result<([String; SLOTS], String)> {
    ensure!(text.len() < 1_000_000, "Too large for a keyword set");
    let Node::Record(record) = lua::template(text)? else {
        bail!("Not a keyword set");
    };
    let text_of = |n: Option<&Node>| match n {
        Some(Node::Text(t)) => Some(lua::localized(t).trim().to_string()),
        _ => None,
    };
    if let Some(kind) = text_of(record.get("type")) {
        ensure!(kind == "KeywordSet", "Not a keyword set: {kind}");
    }
    let name = text_of(record.get("title"))
        .filter(|t| !t.is_empty())
        .or_else(|| text_of(record.get("internalName")).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| file_stem.to_string());
    let mut keywords: [String; SLOTS] = Default::default();
    if let Some(Node::Record(value)) = record.get("value") {
        for (i, slot) in keywords.iter_mut().enumerate() {
            *slot = text_of(value.get(&format!("shortcut{}title", i + 1))).unwrap_or_default();
        }
    }
    Ok((keywords, name))
}

/// A keyword set file as Lightroom writes one: its keys sorted, a tab per
/// level, and a comma after every entry.
pub fn write(name: &str, keywords: &[String; SLOTS]) -> String {
    let id = crate::xmp::preset_write::new_uuid();
    let id = format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    );
    let mut out = String::from("s = {\n");
    out.push_str(&format!("\tid = {},\n", lua::quoted(&id)));
    out.push_str(&format!("\tinternalName = {},\n", lua::quoted(name)));
    out.push_str(&format!("\ttitle = {},\n", lua::quoted(name)));
    out.push_str("\ttype = \"KeywordSet\",\n\tvalue = {\n");
    for (i, k) in keywords.iter().enumerate() {
        out.push_str(&format!(
            "\t\tshortcut{}title = {},\n",
            i + 1,
            lua::quoted(k.trim())
        ));
    }
    out.push_str("\t},\n\tversion = 0,\n}\n");
    out
}

/// The sets of RAWmakase's folder and Lightroom's, both as the caller gives
/// them. Without its own folder only the built-in sets and Lightroom's are
/// listed, and none can be saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeywordSets {
    /// RAWmakase's own sets.
    pub dir: Option<PathBuf>,
    /// Lightroom Classic's, where this computer has a place for them.
    pub lightroom: Option<PathBuf>,
}

/// The sets found, by name, and the files that could not be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeywordSetList {
    pub sets: Vec<KeywordSet>,
    pub errors: Vec<String>,
}

impl KeywordSetList {
    /// The set named `name`, ignoring case.
    pub fn get(&self, name: &str) -> Option<&KeywordSet> {
        let key = name.trim().to_lowercase();
        self.sets.iter().find(|s| s.name.to_lowercase() == key)
    }
}

impl KeywordSets {
    /// Every set, by name. Of sets of the same name, ignoring case,
    /// RAWmakase's own is listed, else Lightroom's, else the built-in one.
    pub fn list(&self) -> KeywordSetList {
        let mut list = KeywordSetList::default();
        let mut found = Vec::new();
        if let Some(dir) = &self.dir {
            read_dir(dir, Origin::Own, &mut found, &mut list.errors);
        }
        if let Some(dir) = &self.lightroom {
            read_dir(dir, Origin::Lightroom, &mut found, &mut list.errors);
        }
        let built_in = BUILT_IN.iter().map(|(name, keywords)| KeywordSet {
            name: name.to_string(),
            keywords: keywords.map(str::to_string),
            origin: Origin::BuiltIn,
        });
        for set in found.into_iter().map(|(set, _)| set).chain(built_in) {
            if list.get(&set.name).is_none() {
                list.sets.push(set);
            }
        }
        list.sets
            .sort_by_key(|s| (s.name.to_lowercase(), s.name.clone()));
        list
    }
    /// Saves `keywords` as RAWmakase's set `name`, replacing one of its own
    /// of that name when `replace`; a set of Lightroom's of that name is left
    /// as it is and is listed no more. Returns its file.
    pub fn save(&self, name: &str, keywords: &[String; SLOTS], replace: bool) -> Result<PathBuf> {
        let name = name.trim();
        ensure!(!name.is_empty(), "A keyword set needs a name");
        let own = self.own(name);
        ensure!(
            replace || own.is_none(),
            "A keyword set named \"{name}\" is already saved"
        );
        std::fs::create_dir_all(self.own_dir()?)?;
        let text = write(name, keywords);
        // Only the set's own file is replaced; a new set takes a file no
        // other set has, as two names can make one file name ("A/B", "A:B").
        let (target, how) = match own {
            Some(path) => (path, Replace::Overwrite),
            None => (self.free_path(name)?, Replace::NoClobber),
        };
        crate::storage::write_atomic(&target, how, |f| {
            use std::io::Write;
            Ok(f.write_all(text.as_bytes())?)
        })?;
        Ok(target)
    }
    /// Renames RAWmakase's set `from` to `to`, keeping its keywords.
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        let to = to.trim();
        ensure!(!to.is_empty(), "A keyword set needs a name");
        let old = self
            .own(from)
            .context("Only RAWmakase's own keyword sets can be renamed")?;
        let text = std::fs::read_to_string(&old)?;
        let (keywords, _) = parse(&text, from)?;
        let same = from.trim().eq_ignore_ascii_case(to);
        ensure!(
            same || self.own(to).is_none(),
            "A keyword set named \"{to}\" is already saved"
        );
        let (new, how) = if same {
            (old.clone(), Replace::Overwrite)
        } else {
            (self.free_path(to)?, Replace::NoClobber)
        };
        crate::storage::write_atomic(&new, how, |f| {
            use std::io::Write;
            Ok(f.write_all(write(to, &keywords).as_bytes())?)
        })?;
        if new != old {
            std::fs::remove_file(&old)?;
        }
        Ok(())
    }
    /// Deletes RAWmakase's set `name`.
    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self
            .own(name)
            .context("Only RAWmakase's own keyword sets can be deleted")?;
        std::fs::remove_file(path)?;
        Ok(())
    }
    /// RAWmakase's own folder, which a set is saved in.
    fn own_dir(&self) -> Result<&Path> {
        self.dir
            .as_deref()
            .context("There is no data folder to keep keyword sets in")
    }
    /// A file for a new set named `name` that no file has yet: its name as
    /// a file name, else with " 2", " 3" and so on.
    fn free_path(&self, name: &str) -> Result<PathBuf> {
        let stem = crate::presets::user::file_name(name);
        ensure!(!stem.is_empty(), "That name can't be a file name");
        // Whatever the case of a name already there, as a Mac's or Windows'
        // disk compares them.
        let dir = self.own_dir()?;
        let taken: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_lowercase())
            .collect();
        (1..1000)
            .map(|n| match n {
                1 => format!("{stem}.lrtemplate"),
                n => format!("{stem} {n}.lrtemplate"),
            })
            .find(|file| !taken.contains(&file.to_lowercase()))
            .map(|file| dir.join(file))
            .context("Too many keyword sets of that name")
    }
    /// The file of RAWmakase's own set named `name`, ignoring case.
    fn own(&self, name: &str) -> Option<PathBuf> {
        let mut found = Vec::new();
        read_dir(
            self.dir.as_deref()?,
            Origin::Own,
            &mut found,
            &mut Vec::new(),
        );
        let key = name.trim().to_lowercase();
        found
            .into_iter()
            .find(|s| s.0.name.to_lowercase() == key)
            .map(|s| s.1)
    }
}

/// The sets in `dir`, with their files.
fn read_dir(
    dir: &Path,
    origin: Origin,
    found: &mut Vec<(KeywordSet, PathBuf)>,
    errors: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            !crate::storage::is_hidden(p)
                && p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("lrtemplate"))
        })
        .collect();
    paths.sort();
    for path in paths {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| parse(&text, &stem))
        {
            Ok((keywords, name)) => found.push((
                KeywordSet {
                    name,
                    keywords,
                    origin,
                },
                path,
            )),
            Err(e) => errors.push(format!("{}: {e:#}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The start of Lightroom Classic's own `Portrait Photography.lrtemplate`.
    const LIGHTROOM: &str = "s = {\n\ttitle = ZSTR \"$$$/AgLibrary/KeywordSetFactoryDefaults/PortraitPhotography=Portrait Photography\",\n\tinternalName = \"Portrait Photography\",\n\ttype = \"KeywordSet\",\n\tvalue = {\n\t\tshortcut1title = ZSTR \"$$$/AgLibrary/KeywordSetFactoryDefaults/PortraitPhotography/Kids=Kids\",\n\t\tshortcut2title = \"\",\n\t\tshortcut3title = \"Paris < France\",\n\t},\n\tversion = 1,\n}\n";

    fn keywords(names: &[&str]) -> [String; SLOTS] {
        let mut out: [String; SLOTS] = Default::default();
        for (slot, name) in out.iter_mut().zip(names) {
            *slot = name.to_string();
        }
        out
    }

    fn store(dir: &Path) -> KeywordSets {
        KeywordSets {
            dir: Some(dir.join("own")),
            lightroom: Some(dir.join("lightroom")),
        }
    }

    #[test]
    fn without_its_own_folder_the_built_in_sets_are_listed_and_none_saved() {
        let none = KeywordSets::default();
        let list = none.list();
        assert_eq!(list.sets.len(), BUILT_IN.len());
        assert!(list.errors.is_empty());
        let error = none.save("Studio", &keywords(&["a"]), false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "There is no data folder to keep keyword sets in"
        );
        assert!(none.rename("Studio", "Other").is_err());
        assert!(none.delete("Studio").is_err());
    }

    #[test]
    fn lightroom_s_files_are_read_with_their_shown_names() -> Result<()> {
        let (read, name) = parse(LIGHTROOM, "file")?;
        assert_eq!(name, "Portrait Photography");
        assert_eq!(read, keywords(&["Kids", "", "Paris < France"]));
        assert!(parse("s = { type = \"Develop\" }", "x").is_err());
        Ok(())
    }

    #[test]
    fn a_saved_set_reads_back_as_written() -> Result<()> {
        let names = keywords(&["Quote \" and \\", "Zażółć", "", "Child < Parent"]);
        let text = write("Festival \"2026\"", &names);
        assert!(text.starts_with("s = {\n\tid = \""));
        assert!(text.contains("\ttype = \"KeywordSet\",\n\tvalue = {\n\t\tshortcut1title"));
        assert_eq!(parse(&text, "x")?, (names, "Festival \"2026\"".to_string()));
        Ok(())
    }

    #[test]
    fn own_sets_come_before_lightroom_s_and_lightroom_s_are_never_changed() -> Result<()> {
        let d = tempfile::tempdir()?;
        let sets = store(d.path());
        // With neither folder: the built-in ones.
        let names: Vec<String> = sets.list().sets.into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "Outdoor Photography",
                "Portrait Photography",
                "Wedding Photography"
            ]
        );
        let lightroom = d.path().join("lightroom");
        std::fs::create_dir_all(&lightroom)?;
        let file = lightroom.join("Portrait Photography.lrtemplate");
        std::fs::write(&file, LIGHTROOM)?;
        std::fs::write(lightroom.join("Broken.lrtemplate"), "s = {")?;
        let list = sets.list();
        assert_eq!(list.errors.len(), 1);
        let portrait = list.get("portrait photography").unwrap();
        assert_eq!(portrait.origin, Origin::Lightroom);
        assert_eq!(portrait.keywords[0], "Kids");
        assert!(sets.delete("Portrait Photography").is_err());
        assert!(sets.rename("Portrait Photography", "Other").is_err());
        // Saving over it saves RAWmakase's own, listed in its place.
        sets.save("Portrait Photography", &keywords(&["Mine"]), true)?;
        assert_eq!(std::fs::read_to_string(&file)?, LIGHTROOM);
        let list = sets.list();
        let portrait = list.get("Portrait Photography").unwrap();
        assert_eq!(portrait.origin, Origin::Own);
        assert_eq!(portrait.keywords[0], "Mine");
        assert_eq!(list.sets.len(), 3);
        Ok(())
    }

    #[test]
    fn own_sets_are_saved_renamed_and_deleted() -> Result<()> {
        let d = tempfile::tempdir()?;
        let sets = store(d.path());
        sets.save("Festival", &keywords(&["Stage"]), false)?;
        assert!(sets.save("festival", &keywords(&["Other"]), false).is_err());
        sets.save("FESTIVAL", &keywords(&["Crowd"]), true)?;
        let list = sets.list();
        assert_eq!(list.get("Festival").unwrap().keywords[0], "Crowd");
        assert_eq!(list.get("Festival").unwrap().name, "FESTIVAL");
        sets.save("Concert", &keywords(&[]), false)?;
        assert!(sets.rename("Festival", "concert").is_err());
        sets.rename("Festival", "Festival 2026")?;
        let list = sets.list();
        assert!(list.get("Festival").is_none());
        assert_eq!(list.get("Festival 2026").unwrap().keywords[0], "Crowd");
        sets.delete("festival 2026")?;
        assert!(sets.list().get("Festival 2026").is_none());
        // A name that is no file name is still a set's name, and another
        // set making the same file name keeps both.
        sets.save("A/B", &keywords(&["x"]), false)?;
        sets.save("A:B", &keywords(&["y"]), true)?;
        sets.rename("Concert", "A?B")?;
        let list = sets.list();
        assert_eq!(list.get("A/B").unwrap().keywords[0], "x");
        assert_eq!(list.get("A:B").unwrap().keywords[0], "y");
        assert!(list.get("A?B").is_some());
        Ok(())
    }
}
