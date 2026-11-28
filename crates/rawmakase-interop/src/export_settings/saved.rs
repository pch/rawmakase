//! The file-name templates a user saves under a name (Lightroom's Preset >
//! Save Current Settings as New Preset…), listed wherever files are named:
//! Export's Rename To, Rename Photos' File Naming and the Filename Template
//! Editor's Preset. Saved as `filename_templates.json` in the data folder.
//!
//! A template is chosen by value: export.json and rename.json keep its
//! tokens, so renaming, changing or deleting a saved template never changes
//! what a dialog already chose. A file this release can't read is never
//! written over: the templates then can't be changed until it is mended. A
//! template or field a later release adds is kept as it was read.
use super::{BUILTIN, Naming};
use anyhow::{Result, bail};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// A saved template: its name, and its tokens in Lightroom's form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedTemplate {
    pub name: String,
    pub tokens: String,
}

/// The saved templates, as read from their file.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedTemplates {
    path: PathBuf,
    /// The file's top level: `templates` and whatever a later release added.
    file: Map<String, Value>,
}

fn path() -> PathBuf {
    crate::storage::data_dir().join("filename_templates.json")
}

impl SavedTemplates {
    /// The saved templates; none when nothing was saved yet. An error says
    /// why the file can't be read.
    pub fn load() -> Result<Self, String> {
        Self::load_from(&path())
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        let file = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(file))
                    if file
                        .get("templates")
                        .is_none_or(|t| matches!(t, Value::Array(_))) =>
                {
                    file
                }
                Ok(_) => return Err(format!("{} is not a list of templates", path.display())),
                Err(e) => return Err(format!("{} can't be read: {e}", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(e) => return Err(format!("{} can't be read: {e}", path.display())),
        };
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    fn entries(&self) -> &[Value] {
        match self.file.get("templates") {
            Some(Value::Array(entries)) => entries,
            _ => &[],
        }
    }

    /// The templates, by name (case aside, then as written).
    pub fn list(&self) -> Vec<SavedTemplate> {
        let mut list: Vec<SavedTemplate> = self.entries().iter().filter_map(template).collect();
        list.sort_by(|a, b| {
            key(&a.name)
                .cmp(&key(&b.name))
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    /// The template named `name`, if any.
    pub fn get(&self, name: &str) -> Option<SavedTemplate> {
        self.list().into_iter().find(|t| t.name == name)
    }

    /// The name of the one template whose tokens are `tokens`; `None` when
    /// none or several are.
    pub fn name_of(&self, tokens: &str) -> Option<String> {
        let mut named = self.list().into_iter().filter(|t| t.tokens == tokens);
        match (named.next(), named.next()) {
            (Some(one), None) => Some(one.name),
            _ => None,
        }
    }

    /// Why `name` can't be a new template's name (or `renaming`'s new one),
    /// if it can't.
    pub fn name_problem(&self, name: &str, renaming: Option<&str>) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return Some("A preset needs a name".into());
        }
        let builtin = Naming::ALL
            .iter()
            .map(|n| n.label())
            .chain(BUILTIN.iter().map(|b| b.name))
            .any(|b| key(b) == key(name));
        if builtin {
            return Some(format!("“{name}” is a built-in preset"));
        }
        let taken = self
            .list()
            .iter()
            .any(|t| key(&t.name) == key(name) && Some(t.name.as_str()) != renaming);
        taken.then(|| format!("A preset is already called “{name}”"))
    }

    /// Saves `tokens` as a new template `name`.
    pub fn add(&mut self, name: &str, tokens: &str) -> Result<()> {
        self.reload()?;
        if let Some(problem) = self.name_problem(name, None) {
            bail!(problem);
        }
        let mut entry = Map::new();
        entry.insert("name".into(), name.trim().into());
        entry.insert("tokens".into(), tokens.into());
        self.change(|entries| {
            entries.push(Value::Object(entry));
            Ok(())
        })
    }

    /// Gives template `name` the tokens `tokens`.
    pub fn update(&mut self, name: &str, tokens: &str) -> Result<()> {
        self.reload()?;
        self.change(|entries| {
            let entry = find(entries, name)?;
            entry.insert("tokens".into(), tokens.into());
            Ok(())
        })
    }

    /// Calls template `name` `new_name`.
    pub fn rename(&mut self, name: &str, new_name: &str) -> Result<()> {
        self.reload()?;
        if let Some(problem) = self.name_problem(new_name, Some(name)) {
            bail!(problem);
        }
        self.change(|entries| {
            let entry = find(entries, name)?;
            entry.insert("name".into(), new_name.trim().into());
            Ok(())
        })
    }

    /// Deletes template `name`.
    pub fn delete(&mut self, name: &str) -> Result<()> {
        self.reload()?;
        self.change(|entries| {
            let before = entries.len();
            entries.retain(|e| template(e).is_none_or(|t| t.name != name));
            if entries.len() == before {
                return Err(rawmakase_model::error::Missing::new(format!(
                    "No preset is called “{name}”"
                ))
                .into());
            }
            Ok(())
        })
    }

    /// The templates as their file has them now: another window may have
    /// saved one since they were read. A file that can't be read now is
    /// not written over.
    fn reload(&mut self) -> Result<()> {
        self.file = Self::load_from(&self.path)
            .map_err(anyhow::Error::msg)?
            .file;
        Ok(())
    }

    /// Changes the templates by `change` and saves them; they change here
    /// only once saved.
    fn change(&mut self, change: impl FnOnce(&mut Vec<Value>) -> Result<()>) -> Result<()> {
        let mut file = self.file.clone();
        let mut entries = match file.remove("templates") {
            Some(Value::Array(entries)) => entries,
            _ => Vec::new(),
        };
        change(&mut entries)?;
        file.insert("templates".into(), Value::Array(entries));
        crate::storage::atomic_json(&self.path, &Value::Object(file.clone()))?;
        self.file = file;
        Ok(())
    }
}

/// An entry this release reads as a template.
fn template(entry: &Value) -> Option<SavedTemplate> {
    Some(SavedTemplate {
        name: entry.get("name")?.as_str()?.to_string(),
        tokens: entry.get("tokens")?.as_str()?.to_string(),
    })
}

fn find<'a>(entries: &'a mut [Value], name: &str) -> Result<&'a mut Map<String, Value>> {
    entries
        .iter_mut()
        .filter(|e| template(e).is_some_and(|t| t.name == name))
        .find_map(Value::as_object_mut)
        .ok_or_else(|| {
            rawmakase_model::error::Missing::new(format!("No preset is called “{name}”")).into()
        })
}

/// A name as names are compared and sorted.
fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_saved_listed_renamed_and_deleted() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("filename_templates.json");
        // Nothing saved yet.
        let mut saved = SavedTemplates::load_from(&file).unwrap();
        assert!(saved.list().is_empty());
        saved.add(" zoo ", "{{image_name}}")?;
        saved.add("Apple", "{{com.adobe.model}}")?;
        let names = |s: &SavedTemplates| s.list().into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(names(&saved), ["Apple", "zoo"]);
        // Read back as saved.
        let again = SavedTemplates::load_from(&file).unwrap();
        assert_eq!(again.list(), saved.list());
        // Names are unique, case aside, and not the built-in presets'.
        assert!(saved.add("ZOO", "x").is_err());
        assert!(saved.add("custom name - sequence", "x").is_err());
        assert!(saved.add("Date Time - Model", "x").is_err());
        assert!(saved.add("  ", "x").is_err());
        saved.update("zoo", "{{image_name}}-{{date_YYYY}}")?;
        saved.rename("zoo", "Zoo")?;
        assert_eq!(
            saved.get("Zoo").map(|t| t.tokens).as_deref(),
            Some("{{image_name}}-{{date_YYYY}}")
        );
        assert!(saved.rename("Zoo", "apple").is_err());
        saved.delete("Apple")?;
        assert_eq!(names(&SavedTemplates::load_from(&file).unwrap()), ["Zoo"]);
        let missing = saved.delete("Apple").unwrap_err();
        assert!(missing.is::<rawmakase_model::error::Missing>());
        assert_eq!(missing.to_string(), "No preset is called “Apple”");
        let missing = saved.update("Apple", "x").unwrap_err();
        assert!(missing.is::<rawmakase_model::error::Missing>());
        Ok(())
    }

    /// Several templates with the same tokens: none is named by them.
    #[test]
    fn tokens_name_a_template_only_when_one_has_them() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut saved = SavedTemplates::load_from(&dir.path().join("t.json")).unwrap();
        saved.add("A", "{{image_name}}")?;
        assert_eq!(saved.name_of("{{image_name}}").as_deref(), Some("A"));
        saved.add("B", "{{image_name}}")?;
        assert_eq!(saved.name_of("{{image_name}}"), None);
        Ok(())
    }

    /// What a later release added is kept; a file that can't be read is an
    /// error, and is never written over.
    #[test]
    fn a_later_releases_fields_are_kept_and_an_unreadable_file_left_alone() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("filename_templates.json");
        std::fs::write(
            &file,
            r#"{"version": 9, "templates": [
                {"name": "Mine", "tokens": "{{image_name}}", "shared": true},
                {"kind": "folder", "children": []}
            ]}"#,
        )?;
        let mut saved = SavedTemplates::load_from(&file).unwrap();
        assert_eq!(saved.list().len(), 1);
        saved.rename("Mine", "Ours")?;
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(written["version"], 9);
        assert_eq!(written["templates"][0]["shared"], true);
        assert_eq!(written["templates"][0]["name"], "Ours");
        assert_eq!(written["templates"][1]["kind"], "folder");
        std::fs::write(&file, b"{not json")?;
        assert!(SavedTemplates::load_from(&file).is_err());
        std::fs::write(&file, br#"{"templates": 3}"#)?;
        assert!(SavedTemplates::load_from(&file).is_err());
        assert_eq!(std::fs::read(&file)?, br#"{"templates": 3}"#);
        Ok(())
    }

    /// A change starts from the file as it is now: a template another
    /// window saved since is kept, and a file that can't be read any more
    /// is left alone.
    #[test]
    fn a_change_keeps_what_was_saved_since_and_never_writes_over_a_broken_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("filename_templates.json");
        let mut here = SavedTemplates::load_from(&file).unwrap();
        let mut there = SavedTemplates::load_from(&file).unwrap();
        there.add("There", "{{image_name}}")?;
        here.add("Here", "{{com.adobe.model}}")?;
        let names: Vec<String> = SavedTemplates::load_from(&file)
            .unwrap()
            .list()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["Here", "There"]);
        assert!(here.add("there", "x").is_err(), "taken since");
        std::fs::write(&file, b"{broken")?;
        assert!(here.delete("Here").is_err());
        assert_eq!(std::fs::read(&file)?, b"{broken");
        Ok(())
    }

    /// A template that can't be saved is not kept either.
    #[test]
    fn a_failed_save_changes_nothing() -> Result<()> {
        let dir = tempfile::tempdir()?;
        // A folder where the file would go: it can't be written.
        let file = dir.path().join("filename_templates.json");
        std::fs::create_dir(&file)?;
        let mut saved = SavedTemplates {
            path: file,
            file: Map::new(),
        };
        assert!(saved.add("A", "{{image_name}}").is_err());
        assert!(saved.list().is_empty());
        Ok(())
    }
}
