use crate::xmp::{Preset, parse};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
#[derive(Default, Clone)]
pub struct Library {
    pub presets: Vec<Preset>,
    pub errors: Vec<String>,
}
/// The XMP presets installed in a set of folders, beside the built-in ones,
/// and how the user arranged them (their layout file, in the first data
/// folder): what the Presets panel lists. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct PresetLibrary {
    dirs: Vec<PathBuf>,
    layout: Option<PathBuf>,
}
impl PresetLibrary {
    /// The presets in the `xmp-presets` folder of each data folder, where
    /// RAWmakase imports them, and the layout file in the first.
    pub fn in_data_dirs(data_dirs: &[PathBuf]) -> Self {
        Self {
            dirs: data_dirs.iter().map(|d| d.join("xmp-presets")).collect(),
            layout: data_dirs.first().map(|d| d.join("preset-layout.json")),
        }
    }
    /// The built-in presets alone, arranged as they come.
    pub fn none() -> Self {
        Self::default()
    }
    /// The folders searched, in order.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }
    /// Every preset: the built-in ones first, as in Lightroom, then the
    /// installed ones by group and name, with the files that failed to read.
    pub fn load(&self) -> Library {
        load_from(&self.dirs)
    }
    /// How the user arranged the presets; as they come when nothing was saved.
    pub fn load_layout(&self) -> PresetLayout {
        self.layout
            .as_deref()
            .map(crate::storage::read_json_or_default)
            .unwrap_or_default()
    }
    /// Saves how the user arranged the presets.
    pub fn save_layout(&self, layout: &PresetLayout) -> Result<()> {
        let path = self
            .layout
            .as_deref()
            .context("The presets have no data folder to keep their order in")?;
        crate::storage::atomic_json(path, layout)
    }
}
fn load_from(dirs: &[PathBuf]) -> Library {
    let mut library = Library::default();
    let mut paths = Vec::new();
    fn scan(dir: &Path, paths: &mut Vec<PathBuf>, depth: usize) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(kind) = e.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !["Defaults", "GPU", "__MACOSX"]
                    .iter()
                    .any(|name| p.file_name().is_some_and(|n| n == *name))
                {
                    scan(&p, paths, depth + 1);
                }
            } else if kind.is_file() && p.extension().is_some_and(|s| s.eq_ignore_ascii_case("xmp"))
            {
                paths.push(p);
            }
        }
    }
    for dir in dirs {
        scan(dir, &mut paths, 0);
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        let result = (|| -> Result<Preset> {
            ensure!(std::fs::metadata(&path)?.len() < 8_000_000, "XMP too large");
            parse(&path, &std::fs::read_to_string(&path)?)
        })();
        match result {
            Ok(p) => {
                if p.settings.get("ShowInPresets").is_none_or(|v| v != "False") {
                    library.presets.push(p);
                }
            }
            Err(e) => library.errors.push(format!("{}: {e:#}", path.display())),
        }
    }
    library
        .presets
        .sort_by(|a, b| a.group.cmp(&b.group).then(a.name.cmp(&b.name)));
    // Built-in presets come first, as in Lightroom.
    let (builtin, errors) = super::builtin::presets();
    library.presets.splice(0..0, builtin);
    library.errors.extend(errors);
    library
}
/// The preset `id` names (see `Preset::id`): a built-in one, or an installed
/// file. `None` once it is gone or no longer reads.
pub fn find_preset(id: &str) -> Option<Preset> {
    if id.starts_with("builtin:") {
        return super::builtin::presets().0.into_iter().find(|p| p.id == id);
    }
    let path = Path::new(id);
    if std::fs::metadata(path).ok()?.len() >= 8_000_000 {
        return None;
    }
    parse(path, &std::fs::read_to_string(path).ok()?).ok()
}
pub(super) fn favorite_path() -> PathBuf {
    crate::storage::data_dir().join("preset-favorites.json")
}
pub fn load_favorites() -> BTreeSet<String> {
    crate::storage::read_json_or_default(&favorite_path())
}
pub fn save_favorites(favorites: &BTreeSet<String>) -> Result<()> {
    crate::storage::atomic_json(&favorite_path(), favorites)
}
/// How the user arranged the preset list in Manage Presets: the groups' order, by
/// `PresetLayout::group_key`, and the presets hidden from it, by `Preset::id`.
/// Groups not in `order` keep their default place after those that are.
#[derive(Default, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PresetLayout {
    pub order: Vec<String>,
    pub hidden: BTreeSet<String>,
}
impl PresetLayout {
    /// A group's key: built-in and imported groups of the same name stay apart.
    pub fn group_key(imported: bool, name: &str) -> String {
        format!("{}:{name}", if imported { "imported" } else { "builtin" })
    }
    /// Where the group `key` goes; groups never ordered go after every ordered one.
    pub fn rank(&self, key: &str) -> usize {
        self.order
            .iter()
            .position(|k| k == key)
            .unwrap_or(usize::MAX)
    }
}
pub fn display_name(name: &str) -> String {
    name.replace('⁺', "+")
        .replace('⁻', "-")
        .replace('¹', "1")
        .replace('²', "2")
        .replace('³', "3")
}

#[cfg(test)]
mod library_tests {
    use super::*;

    const PRESET: &str = include_str!("../../../../assets/presets/BandW/Neutral.xmp");

    /// Two libraries on two data folders each list their own installed presets
    /// after the built-in ones, and keep their own layout, in their first folder.
    #[test]
    fn each_library_lists_its_own_presets_and_keeps_its_own_order() -> Result<()> {
        let (a, b) = (tempfile::tempdir()?, tempfile::tempdir()?);
        std::fs::create_dir(a.path().join("xmp-presets"))?;
        std::fs::write(a.path().join("xmp-presets/Mine.xmp"), PRESET)?;
        let first = PresetLibrary::in_data_dirs(&[a.path().to_path_buf()]);
        let second = PresetLibrary::in_data_dirs(&[b.path().to_path_buf()]);
        let builtin = PresetLibrary::none().load().presets.len();
        assert!(builtin > 0);
        assert_eq!(first.load().presets.len(), builtin + 1);
        assert_eq!(second.load().presets.len(), builtin);
        let layout = PresetLayout {
            order: vec![PresetLayout::group_key(true, "Mine")],
            ..Default::default()
        };
        first.save_layout(&layout)?;
        assert!(a.path().join("preset-layout.json").is_file());
        assert_eq!(first.load_layout(), layout);
        assert_eq!(second.load_layout(), PresetLayout::default());
        assert!(PresetLibrary::none().save_layout(&layout).is_err());
        Ok(())
    }
}
