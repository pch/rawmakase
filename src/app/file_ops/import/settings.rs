//! Import Photos' choices, kept for the next import in `import-copy.json`
//! beside `import-options.json` (Preferences' choices for every photo
//! added), with what a later release adds to it saved back untouched.
use crate::catalog::file_ops::{DateFolders, ExtensionCase, Organize};
use crate::export_settings::{LaterFields, RenameSettings};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(in crate::app) struct ImportCopySettings {
    /// The folder last imported from, offered when no card is attached.
    pub source: Option<PathBuf>,
    /// The source's subfolders are read too.
    pub subfolders: bool,
    /// The folder the photos go into, or below.
    pub destination: Option<PathBuf>,
    /// Lightroom's Into Subfolder, and its name, kept while it is off.
    pub into_subfolder: bool,
    pub subfolder: String,
    /// A choice only a later release knows reads as the default.
    #[serde(deserialize_with = "organize_or_default")]
    pub organize: Organize,
    /// The photos are named by `naming` (Rename Files); else they keep
    /// their names.
    pub rename: bool,
    /// The template, Custom Text and Start Number, as Rename Photos keeps
    /// them.
    pub naming: RenameSettings,
    pub shoot_name: String,
    #[serde(deserialize_with = "extensions_or_default")]
    pub extensions: ExtensionCase,
    /// Lightroom's Don't Import Suspected Duplicates.
    pub skip_duplicates: bool,
    /// Apply During Import, as typed: Creator, Copyright and Keywords
    /// (", " between them, as the Keywording panel takes them).
    pub creator: String,
    pub copyright: String,
    pub keywords: String,
    #[serde(flatten)]
    pub later_fields: LaterFields,
}

impl Default for ImportCopySettings {
    fn default() -> Self {
        Self {
            source: None,
            subfolders: true,
            destination: None,
            into_subfolder: false,
            subfolder: String::new(),
            // Lightroom's own first choice.
            organize: Organize::ByDate(DateFolders::YearThenDate),
            rename: false,
            naming: RenameSettings::default(),
            shoot_name: String::new(),
            extensions: ExtensionCase::LeaveAsIs,
            // As the issue asks; Lightroom keeps the last choice, and its
            // first one wasn't measured.
            skip_duplicates: true,
            creator: String::new(),
            copyright: String::new(),
            keywords: String::new(),
            later_fields: LaterFields::default(),
        }
    }
}

fn organize_or_default<'de, D>(d: D) -> Result<Organize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_else(|_| ImportCopySettings::default().organize))
}

/// A case only a later release knows reads as Leave as-is, keeping the
/// rest of the choices.
fn extensions_or_default<'de, D>(d: D) -> Result<ExtensionCase, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[cfg_attr(test, allow(dead_code))]
fn path() -> PathBuf {
    crate::storage::data_dir().join("import-copy.json")
}

// Tests keep the choices in `FileOps` instead.
#[cfg_attr(test, allow(dead_code))]
impl ImportCopySettings {
    /// The last import's choices, else the defaults.
    pub(in crate::app) fn load() -> Self {
        Self::load_from(&path())
    }
    pub(in crate::app) fn save(&self) -> anyhow::Result<()> {
        self.save_to(&path())
    }
    fn load_from(path: &Path) -> Self {
        crate::storage::read_json_or_default(path)
    }
    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        crate::storage::atomic_json(path, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_round_trip_keeping_what_a_later_release_added() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("import-copy.json");
        assert_eq!(
            ImportCopySettings::load_from(&file),
            ImportCopySettings::default()
        );
        std::fs::write(
            &file,
            r#"{"subfolders": false, "destination": "/photos",
                "organize": {"ByDate": "%Y/%m"}, "rename": true,
                "into_subfolder": true, "subfolder": "Shoot", "skip_duplicates": false,
                "copyright": "© Ann", "keywords": "trip, Places > Kraków",
                "naming": {"tokens": "{{naming_originalFilename}}", "start_number": 7},
                "eject_after": true}"#,
        )?;
        let read = ImportCopySettings::load_from(&file);
        assert!(!read.subfolders);
        assert_eq!(read.destination.as_deref(), Some(Path::new("/photos")));
        assert_eq!(read.organize, Organize::ByDate(DateFolders::YearThenMonth));
        assert!(read.into_subfolder && !read.skip_duplicates);
        assert_eq!(read.subfolder, "Shoot");
        assert_eq!(read.copyright, "© Ann");
        assert!(read.rename);
        assert_eq!(read.naming.start_number, 7);
        read.save_to(&file)?;
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(saved["eject_after"], true);
        assert_eq!(ImportCopySettings::load_from(&file), read);
        Ok(())
    }

    #[test]
    fn an_organize_choice_of_a_later_release_reads_as_the_default() {
        let read: ImportCopySettings = serde_json::from_str(
            r#"{"organize": {"ByPattern": "{year}/{camera}"}, "rename": true,
                "extensions": "TitleCase"}"#,
        )
        .unwrap();
        assert_eq!(read.organize, ImportCopySettings::default().organize);
        assert_eq!(read.extensions, ExtensionCase::LeaveAsIs);
        assert!(read.rename, "the rest is kept");
    }
}
