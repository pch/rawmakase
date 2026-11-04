use crate::xmp::{Preset, parse};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
#[derive(Default, Clone)]
pub struct Library {
    pub presets: Vec<Preset>,
    pub errors: Vec<String>,
}
pub fn library_dirs() -> Vec<PathBuf> {
    crate::storage::asset_dirs()
        .into_iter()
        .map(|p| p.join("xmp-presets"))
        .collect()
}
pub fn load_library() -> Library {
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
    for dir in library_dirs() {
        scan(&dir, &mut paths, 0);
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
pub fn display_name(name: &str) -> String {
    name.replace('⁺', "+")
        .replace('⁻', "-")
        .replace('¹', "1")
        .replace('²', "2")
        .replace('³', "3")
}
