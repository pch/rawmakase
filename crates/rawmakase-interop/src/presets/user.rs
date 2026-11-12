//! Presets made in RAWmakase: XMP files in the user library's "User Presets" folder,
//! one folder per group, which Lightroom and Camera Raw can read too. Presets
//! imported into the user library ("Imported", copies of the chosen files) can be
//! updated, renamed and deleted too, as in Lightroom; the first update keeps the
//! imported file in "preset-backups", outside the library. Built-in presets and those
//! installed anywhere else cannot be changed.
use crate::{camera_data::Metadata, model::recipe::Recipe};
use crate::{
    model::settings_groups::{GroupInclusion, GroupSelection},
    storage::Replace,
    xmp::{
        Preset,
        preset_write::{PresetInfo, group_of_key, preset},
    },
};
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};

/// The folders of presets made in RAWmakase and of imported ones, under the user
/// library, and where imported presets are kept as they were before an update.
#[derive(Clone, Debug, PartialEq)]
pub struct UserPresets {
    pub dir: PathBuf,
    pub imported: PathBuf,
    pub backups: PathBuf,
}

impl Default for UserPresets {
    fn default() -> Self {
        let data = crate::storage::data_dir();
        let library = data.join("xmp-presets");
        Self {
            dir: library.join("User Presets"),
            imported: library.join("Imported"),
            // Outside the library, so a backup never shows as a preset of its own.
            backups: data.join("preset-backups"),
        }
    }
}

impl UserPresets {
    /// Whether `preset` was made in RAWmakase, so it may be changed here.
    /// Only files RAWmakase wrote count: they carry its marker, so a preset copied
    /// into the folder from elsewhere is never updated or deleted here.
    pub fn owns(&self, preset: &Preset) -> bool {
        preset.path.starts_with(&self.dir)
            && preset
                .settings
                .get("RAWmakasePreset")
                .is_some_and(|v| matches!(v.as_str(), "1" | "2" | "3"))
    }
    /// Whether `preset` may be updated, renamed or deleted here: made here, or a
    /// copy that Import made in the user library.
    pub fn manages(&self, preset: &Preset) -> bool {
        self.owns(preset) || (!preset.builtin && preset.path.starts_with(&self.imported))
    }
    /// Saves a new preset of `r`'s `groups`, edited on the photo `shown`; returns its
    /// file.
    pub fn create(
        &self,
        r: &Recipe,
        shown: &Metadata,
        info: &PresetInfo,
        groups: &GroupSelection,
    ) -> Result<PathBuf> {
        ensure!(!info.name.is_empty(), "A preset needs a name");
        ensure!(!groups.is_empty(), "Choose at least one setting");
        let path = self.file_for(info)?;
        write(&path, Replace::NoClobber, &preset(r, shown, info, groups)).with_context(|| {
            format!("A preset named {} is already in {}", info.name, info.group)
        })?;
        Ok(path)
    }
    /// Lightroom's Update with Current Settings: the preset keeps its name, group,
    /// identity and the setting groups it had. Only the settings RAWmakase writes are
    /// kept, so an imported preset is first copied as it was to `backups`, once:
    /// later updates keep that first, imported file. Returns the copy made, if any.
    pub fn update(
        &self,
        existing: &Preset,
        r: &Recipe,
        shown: &Metadata,
    ) -> Result<Option<PathBuf>> {
        ensure!(
            self.manages(existing),
            "Only presets made here or imported can be updated"
        );
        let groups = groups_of(existing);
        ensure!(
            !groups.is_empty(),
            "This preset holds no settings RAWmakase can update"
        );
        let backup = match existing.path.strip_prefix(&self.imported) {
            Ok(within) if !self.backed_up(existing, within) => Some(
                self.back_up(existing, within)
                    .context("The preset was not updated: its original couldn't be kept")?,
            ),
            _ => None,
        };
        let info = info_of(existing);
        write(
            &existing.path,
            Replace::Overwrite,
            &preset(r, shown, &info, &groups),
        )?;
        Ok(backup)
    }
    /// Whether the imported preset `existing`, at `within` under `imported`, was kept
    /// before an earlier update: it has been updated here, and the backup at its
    /// place is the same preset, by its UUID or, for one without (which an update
    /// gives a new one), its name. Another preset later imported or renamed to the
    /// same place is not.
    fn backed_up(&self, existing: &Preset, within: &Path) -> bool {
        let backup = self.backups.join(within);
        let uuid = |p: &Preset| p.settings.get("UUID").cloned();
        existing.settings.contains_key("RAWmakasePreset")
            && std::fs::read_to_string(&backup)
                .ok()
                .and_then(|text| crate::xmp::parse(&backup, &text).ok())
                .is_some_and(|b| match uuid(&b) {
                    Some(id) => Some(id) == uuid(existing),
                    None => b.name == existing.name,
                })
    }
    /// Copies `existing` as it is to `within` under `backups`, or a numbered name
    /// beside it when that is taken; returns the copy.
    fn back_up(&self, existing: &Preset, within: &Path) -> Result<PathBuf> {
        use std::io::Write;
        let original = std::fs::read(&existing.path)?;
        let first = self.backups.join(within);
        let dir = crate::storage::parent_dir(&first).to_path_buf();
        let stem = first
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        std::fs::create_dir_all(&dir)?;
        let mut backup = first;
        let mut n = 2;
        while backup.exists() {
            backup = dir.join(format!("{stem} {n}.xmp"));
            n += 1;
        }
        crate::storage::write_atomic(&backup, Replace::NoClobber, |f| Ok(f.write_all(&original)?))?;
        Ok(backup)
    }
    /// Renames a preset, keeping its settings and identity; returns its new file.
    /// A preset made here moves to its group's folder under the new name; an
    /// imported one stays in its folder.
    pub fn rename(&self, existing: &Preset, name: &str) -> Result<PathBuf> {
        ensure!(
            self.manages(existing),
            "Only presets made here or imported can be renamed"
        );
        let info = PresetInfo {
            name: name.trim().to_string(),
            ..info_of(existing)
        };
        ensure!(!info.name.is_empty(), "A preset needs a name");
        let path = if self.owns(existing) {
            self.file_for(&info)?
        } else {
            let stem = file_name(&info.name);
            ensure!(!stem.is_empty(), "A preset needs a name");
            crate::storage::parent_dir(&existing.path).join(format!("{stem}.xmp"))
        };
        let text = std::fs::read_to_string(&existing.path)?;
        let renamed = with_name(&text, &info.name)?;
        // Checked before anything is written: a file that no longer reads, or reads
        // as another name (one only its file names, with characters a file name
        // can't hold), would lose the preset with its old file.
        ensure!(
            crate::xmp::parse(&path, &renamed).is_ok_and(|p| p.name == info.name),
            "This preset can't be renamed to {}",
            info.name
        );
        // The same file whatever the case of its letters, as on macOS and Windows:
        // write it in place, then let the rename fix the case.
        // Only when both names lead to this very file, as on a case-insensitive file
        // system; on Linux they can be two presets, and the other must not go.
        let resolved = |p: &Path| std::fs::canonicalize(p).ok();
        let same_file = path == existing.path
            || resolved(&path).is_some_and(|p| Some(p) == resolved(&existing.path));
        if same_file {
            write(&existing.path, Replace::Overwrite, &renamed)?;
            std::fs::rename(&existing.path, &path)?;
        } else {
            write(&path, Replace::NoClobber, &renamed).with_context(|| {
                format!("A preset named {} is already in {}", info.name, info.group)
            })?;
            std::fs::remove_file(&existing.path)?;
        }
        Ok(path)
    }
    /// Deletes a preset made here or imported.
    pub fn delete(&self, existing: &Preset) -> Result<()> {
        ensure!(
            self.manages(existing),
            "Only presets made here or imported can be deleted"
        );
        std::fs::remove_file(&existing.path)?;
        Ok(())
    }
    /// `dir/<group>/<name>.xmp`, with characters file systems refuse replaced.
    fn file_for(&self, info: &PresetInfo) -> Result<PathBuf> {
        let group = match file_name(&info.group) {
            g if g.is_empty() => "User Presets".to_string(),
            g => g,
        };
        let name = file_name(&info.name);
        ensure!(!name.is_empty(), "A preset needs a name");
        Ok(self.dir.join(group).join(format!("{name}.xmp")))
    }
}

/// `s` as a file name, with the characters and names Windows, macOS or Linux refuse
/// in one replaced.
pub(crate) fn file_name(s: &str) -> String {
    let name: String = s
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_start_matches('.').trim_end_matches(['.', ' ']);
    let reserved = ["CON", "PRN", "AUX", "NUL"].iter().any(|r| {
        name.split('.')
            .next()
            .is_some_and(|n| n.eq_ignore_ascii_case(r))
    }) || ["COM", "LPT"].iter().any(|r| {
        let stem = name.split('.').next().unwrap_or("");
        let stem = stem.as_bytes();
        stem.len() == 4 && stem[..3].eq_ignore_ascii_case(r.as_bytes()) && stem[3].is_ascii_digit()
    });
    if reserved {
        format!("_{name}")
    } else {
        name.to_string()
    }
}

/// A preset file's `text` with its name, every language of it, set to `name`, and
/// nothing else changed. A file without a name of its own is named by its file
/// alone, so it comes back as it was.
fn with_name(text: &str, name: &str) -> Result<String> {
    use crate::xml::ns::{CRS, RDF};
    let doc = roxmltree::Document::parse(text).context("This preset's name can't be changed")?;
    // The preset's own description, as `xmp::parse` finds it, not a Look's inside.
    let description = doc
        .descendants()
        .find(|n| {
            n.has_tag_name((RDF, "Description"))
                && (n.attributes().any(|a| a.namespace() == Some(CRS))
                    || n.children().any(|c| c.tag_name().namespace() == Some(CRS)))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .context("No top-level XMP description")?;
    let mut edits = Vec::new();
    if let Some(attribute) = description
        .attributes()
        .find(|a| a.namespace() == Some(CRS) && a.name() == "Name")
    {
        edits.push((attribute.range_value(), crate::xml::escape_attribute(name)));
    }
    if let Some(element) = description
        .children()
        .find(|n| n.has_tag_name((CRS, "Name")))
    {
        let items: Vec<_> = element
            .descendants()
            .filter(|n| n.has_tag_name((RDF, "li")))
            .collect();
        let holders = if items.is_empty() {
            vec![element]
        } else {
            items
        };
        for holder in holders {
            // An empty item has no text to replace; leave the file alone.
            let text = holder
                .first_child()
                .filter(|t| t.is_text() && holder.children().count() == 1)
                .context("This preset's name can't be changed")?;
            edits.push((text.range(), crate::xml::escape_text(name)));
        }
    }
    // From the end, so earlier ranges stay where they were.
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = text.to_string();
    for (range, value) in edits {
        out.replace_range(range, &value);
    }
    Ok(out)
}

/// The setting groups a preset holds, from the keys it sets.
pub fn groups_of(preset: &Preset) -> GroupSelection {
    let mut groups = GroupSelection::none();
    // Panel switches go with whichever groups were chosen, so they say nothing here.
    for key in preset
        .settings
        .keys()
        .chain(preset.curves.keys())
        .filter(|k| !k.starts_with("Enable"))
    {
        if let Some(group) = group_of_key(key) {
            groups.set(group, GroupInclusion::Included);
        }
    }
    // A Look is written with the profile, which an imported preset may not name.
    if !preset.look.is_empty()
        && let Some(group) = group_of_key("CameraProfile")
    {
        groups.set(group, GroupInclusion::Included);
    }
    groups
}

fn info_of(p: &Preset) -> PresetInfo {
    PresetInfo {
        name: p.name.clone(),
        group: p.group.clone(),
        uuid: p
            .settings
            .get("UUID")
            .cloned()
            .unwrap_or_else(|| PresetInfo::new(&p.name, &p.group).uuid),
    }
}

fn write(path: &Path, replace: Replace, text: &str) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(crate::storage::parent_dir(path))?;
    crate::storage::write_atomic(path, replace, |f| Ok(f.write_all(text.as_bytes())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(dir: &Path) -> crate::presets::Library {
        let mut presets = Vec::new();
        for entry in walk(dir) {
            presets.push(
                crate::xmp::parse(&entry, &std::fs::read_to_string(&entry).unwrap()).unwrap(),
            );
        }
        crate::presets::Library {
            presets,
            errors: Vec::new(),
        }
    }
    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn presets_made_here_are_created_updated_renamed_and_deleted() -> Result<()> {
        let d = tempfile::tempdir()?;
        let user = UserPresets {
            dir: d.path().join("User Presets"),
            imported: d.path().join("Imported"),
            backups: d.path().join("preset-backups"),
        };
        let mut groups = GroupSelection::none();
        groups.set(
            crate::model::settings_groups::SettingGroup::Exposure,
            GroupInclusion::Included,
        );
        let r = Recipe {
            exposure: 0.4,
            ..Default::default()
        };
        let shown = Metadata::default();
        let info = PresetInfo::new("Bright/airy", "Mine");
        let path = user.create(&r, &shown, &info, &groups)?;
        assert_eq!(path, user.dir.join("Mine").join("Bright-airy.xmp"));
        // The same name in the same group is refused.
        assert!(user.create(&r, &shown, &info, &groups).is_err());
        assert!(
            user.create(
                &r,
                &shown,
                &PresetInfo::new("Empty", "Mine"),
                &GroupSelection::none()
            )
            .is_err()
        );
        let made = library(&user.dir).presets.remove(0);
        assert!(user.owns(&made));
        assert_eq!(made.name, "Bright/airy");
        // The panel switches written with it don't count as groups of their own.
        assert_eq!(groups_of(&made), groups);
        // Update keeps the groups it had: still only Exposure.
        user.update(
            &made,
            &Recipe {
                exposure: 1.,
                contrast: 0.5,
                ..Default::default()
            },
            &shown,
        )?;
        let updated = library(&user.dir).presets.remove(0);
        assert_eq!(updated.settings["Exposure2012"], "+1.00");
        // A preset made here has no original to keep.
        assert!(!user.backups.exists());
        assert!(!updated.settings.contains_key("Contrast2012"));
        assert_eq!(updated.settings["UUID"], info.uuid);
        // Rename moves the file and keeps the rest.
        let renamed = user.rename(&updated, "Brighter")?;
        let presets = library(&user.dir).presets;
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Brighter");
        assert_eq!(presets[0].path, renamed);
        assert_eq!(presets[0].settings["UUID"], info.uuid);
        user.delete(&presets[0])?;
        assert!(library(&user.dir).presets.is_empty());
        // A case-only rename keeps the one file.
        let created = user.create(&r, &shown, &PresetInfo::new("lower", "Mine"), &groups)?;
        let lower = library(&user.dir).presets.remove(0);
        let upper = user.rename(&lower, "Lower")?;
        assert_eq!(library(&user.dir).presets.len(), 1);
        assert_eq!(library(&user.dir).presets[0].name, "Lower");
        assert_ne!(created, upper);
        user.delete(&library(&user.dir).presets[0])?;
        // Windows-unsafe names are made safe.
        let odd = user.create(&r, &shown, &PresetInfo::new("x? <b>.", "a|b"), &groups)?;
        assert_eq!(odd, user.dir.join("a-b").join("x- -b-.xmp"));
        let reserved = user.create(&r, &shown, &PresetInfo::new("CON", "a|b"), &groups)?;
        assert_eq!(reserved, user.dir.join("a-b").join("_CON.xmp"));
        // Names of four bytes but fewer characters are names like any other.
        for name in ["éé", "🙂", "COMé"] {
            let made = user.create(&r, &shown, &PresetInfo::new(name, "a|b"), &groups)?;
            assert_eq!(made, user.dir.join("a-b").join(format!("{name}.xmp")));
        }
        let com = user.create(&r, &shown, &PresetInfo::new("com1.x", "a|b"), &groups)?;
        assert_eq!(com, user.dir.join("a-b").join("_com1.x.xmp"));
        // A file copied into the folder from elsewhere is not ours to change.
        let foreign = crate::xmp::Preset {
            settings: Default::default(),
            ..library(&user.dir).presets[0].clone()
        };
        assert!(!user.owns(&foreign));
        // Presets that aren't ours are left alone.
        let elsewhere = crate::xmp::Preset {
            path: d.path().join("Installed").join("x.xmp"),
            ..presets[0].clone()
        };
        assert!(!user.manages(&elsewhere));
        assert!(user.delete(&elsewhere).is_err());
        Ok(())
    }

    /// A Lightroom preset as Import copies it: a name in two languages, and a Look
    /// with a name of its own.
    const IMPORTED: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:cr="http://ns.adobe.com/camera-raw-settings/1.0/"
    cr:Exposure2012="+0.50">
   <cr:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Warm &amp; Soft</rdf:li>
     <rdf:li xml:lang="de">Warm &amp; Weich</rdf:li>
    </rdf:Alt>
   </cr:Name>
   <cr:Group><rdf:Alt><rdf:li xml:lang="x-default">Pack</rdf:li></rdf:Alt></cr:Group>
   <cr:Look>
    <rdf:Description cr:Name="Adobe Color"/>
   </cr:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#;

    #[test]
    fn imported_presets_are_updated_renamed_and_deleted() -> Result<()> {
        let d = tempfile::tempdir()?;
        let user = UserPresets {
            dir: d.path().join("User Presets"),
            imported: d.path().join("Imported"),
            backups: d.path().join("preset-backups"),
        };
        let folder = user.imported.join("Pack");
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join("warm.xmp"), IMPORTED)?;
        let imported = library(&user.imported).presets.remove(0);
        assert!(user.manages(&imported) && !user.owns(&imported));
        // A rename doesn't need a backup: nothing but the name changes.
        let path = user.rename(&imported, "Warm")?;
        assert!(!user.backups.exists());
        let imported = library(&user.imported).presets.remove(0);
        // Updated as Lightroom does, with the imported file kept outside the library.
        let r = Recipe {
            exposure: 1.,
            ..Default::default()
        };
        let backup = user.update(&imported, &r, &Metadata::default())?.unwrap();
        assert_eq!(backup, user.backups.join("Pack").join("Warm.xmp"));
        let original = IMPORTED
            .replace("Warm &amp; Soft", "Warm")
            .replace("Warm &amp; Weich", "Warm");
        assert_eq!(std::fs::read_to_string(&backup)?, original);
        let updated = library(&user.imported).presets.remove(0);
        assert_eq!(
            (updated.path.as_path(), updated.name.as_str()),
            (path.as_path(), "Warm")
        );
        assert_eq!(updated.settings["Exposure2012"], "+1.00");
        assert!(user.manages(&updated));
        // A second update keeps the first, imported file.
        assert_eq!(
            user.update(&updated, &Recipe::default(), &Metadata::default())?,
            None
        );
        assert_eq!(std::fs::read_to_string(&backup)?, original);
        assert_eq!(
            library(&user.imported).presets[0].settings["Exposure2012"],
            "0.00"
        );
        // Another preset imported to the same place later is kept beside it, not
        // mistaken for the one already kept.
        std::fs::write(&path, IMPORTED)?;
        let other = library(&user.imported).presets.remove(0);
        let second = user.update(&other, &r, &Metadata::default())?.unwrap();
        assert_eq!(second, user.backups.join("Pack").join("Warm 2.xmp"));
        assert_eq!(std::fs::read_to_string(&second)?, IMPORTED);
        assert_eq!(std::fs::read_to_string(&backup)?, original);
        // A preset of nothing RAWmakase writes is not emptied.
        let masks_only = crate::xmp::Preset {
            settings: Default::default(),
            curves: Default::default(),
            look: String::new(),
            ..other.clone()
        };
        assert!(user.update(&masks_only, &r, &Metadata::default()).is_err());
        // A Look keeps the profile group, though the preset names no profile.
        assert!(
            groups_of(&other)
                .contains(crate::model::settings_groups::SettingGroup::TreatmentAndProfile)
        );
        std::fs::write(&path, IMPORTED)?;
        std::fs::rename(&path, folder.join("warm.xmp"))?;
        let imported = library(&user.imported).presets.remove(0);
        // Renamed in its own folder, in every language, keeping the Look's name and
        // the rest of the file as it was.
        let path = user.rename(&imported, " Cool <1> ")?;
        assert_eq!(path, folder.join("Cool -1-.xmp"));
        assert!(!folder.join("warm.xmp").exists());
        assert_eq!(
            std::fs::read_to_string(&path)?,
            IMPORTED
                .replace("Warm &amp; Soft", "Cool &lt;1&gt;")
                .replace("Warm &amp; Weich", "Cool &lt;1&gt;")
        );
        let renamed = library(&user.imported).presets.remove(0);
        assert_eq!(renamed.name, "Cool <1>");
        assert_eq!(renamed.group, "Pack");
        assert_eq!(renamed.look, "Adobe Color");
        // A name given as an attribute is renamed too.
        std::fs::write(
            folder.join("attr.xmp"),
            IMPORTED.replace(
                r#"cr:Exposure2012="+0.50">"#,
                r#"cr:Exposure2012="+0.50" cr:Name="Old">"#,
            ),
        )?;
        let attr = library(&user.imported)
            .presets
            .into_iter()
            .find(|p| p.path.ends_with("attr.xmp"))
            .unwrap();
        let path = user.rename(&attr, "New")?;
        let text = std::fs::read_to_string(&path)?;
        assert!(text.contains(r#"cr:Name="New""#), "{text}");
        assert!(text.contains(r#"cr:Name="Adobe Color""#), "{text}");
        // A name the file couldn't keep leaves the preset as it was.
        let quoted = folder.join("quoted.xmp");
        std::fs::write(
            &quoted,
            IMPORTED.replace(
                r#"cr:Exposure2012="+0.50">"#,
                r#"cr:Exposure2012="+0.50" cr:Name='Old'>"#,
            ),
        )?;
        let before = std::fs::read_to_string(&quoted)?;
        let single = library(&user.imported)
            .presets
            .into_iter()
            .find(|p| p.path == quoted)
            .unwrap();
        assert!(user.rename(&single, "Piotr's").is_err());
        assert_eq!(std::fs::read_to_string(&quoted)?, before);
        assert!(!folder.join("Piotr's.xmp").exists());
        // Another preset's name is not taken over.
        assert!(user.rename(&renamed, "New").is_err());
        assert!(folder.join("Cool -1-.xmp").exists());
        user.delete(&renamed)?;
        assert!(!folder.join("Cool -1-.xmp").exists());
        Ok(())
    }
}
