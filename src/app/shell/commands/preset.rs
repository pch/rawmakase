//! Develop presets as the control API lists and finds them. Only presets in the
//! loaded library can be named: an id is never read as a path.
use super::{Error, Result};
use crate::presets::display_name;
use crate::xmp::Preset;
use serde::Serialize;

/// The preset a `preset` command names.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum PresetTarget {
    /// `id` as `presets` lists it.
    Id(String),
    /// The name as the Presets panel shows it, in any case, or part of it when only
    /// one preset has it; `group` narrows the search to one folder.
    Name { name: String, group: Option<String> },
}

/// A preset as `presets` lists it.
#[derive(Debug, Serialize)]
pub(in crate::app) struct PresetSummary {
    pub id: String,
    pub name: String,
    pub group: String,
    pub builtin: bool,
    /// Why the preset does not fully fit the open photo; `null` when it does or no
    /// photo is open. Applying it still applies the settings that fit.
    pub issue: Option<String>,
}

/// The presets in `group` (any case), or all of them, with the open photo's
/// `issues` in the library's order (empty when no photo is open).
pub(in crate::app) fn summaries(
    presets: &[Preset],
    issues: &[Option<String>],
    group: Option<&str>,
) -> Vec<PresetSummary> {
    presets
        .iter()
        .enumerate()
        .filter(|(_, p)| group.is_none_or(|g| same_group(&p.group, g)))
        .map(|(i, p)| PresetSummary {
            id: p.id.clone(),
            name: display_name(&p.name),
            group: p.group.clone(),
            builtin: p.builtin,
            issue: issues.get(i).cloned().flatten(),
        })
        .collect()
}

/// Whether `asked` names `group`, in any case of any script.
fn same_group(group: &str, asked: &str) -> bool {
    group.to_lowercase() == asked.to_lowercase()
}

/// The index in `presets` of the one preset `target` names.
pub(in crate::app) fn find(presets: &[Preset], target: &PresetTarget) -> Result<usize> {
    let (name, group) = match target {
        PresetTarget::Id(id) => {
            return presets
                .iter()
                .position(|p| p.id == *id)
                .ok_or_else(|| Error::new("not_found", "No preset has this id; list presets"));
        }
        // An empty name is part of every name, so it would pick any lone preset.
        PresetTarget::Name { name, .. } if name.trim().is_empty() => {
            return Err(Error::new("invalid_request", "The preset name is empty"));
        }
        PresetTarget::Name { name, group } => (name.to_lowercase(), group.as_deref()),
    };
    let in_group: Vec<usize> = (0..presets.len())
        .filter(|&i| group.is_none_or(|g| same_group(&presets[i].group, g)))
        .collect();
    let names = |i: usize| {
        let p = &presets[i];
        [p.name.to_lowercase(), display_name(&p.name).to_lowercase()]
    };
    let exact: Vec<usize> = in_group
        .iter()
        .copied()
        .filter(|&i| names(i).contains(&name))
        .collect();
    let matches = if exact.is_empty() {
        in_group
            .into_iter()
            .filter(|&i| names(i).iter().any(|n| n.contains(&name)))
            .collect()
    } else {
        exact
    };
    match matches[..] {
        [] => Err(Error::new("not_found", "No preset matches this name")),
        [i] => Ok(i),
        ref many => {
            let listed: Vec<String> = many
                .iter()
                .take(5)
                .map(|&i| format!("{} / {}", presets[i].group, display_name(&presets[i].name)))
                .collect();
            Err(Error::new(
                "ambiguous",
                format!(
                    "{} presets match ({}{}); add a group or use an id",
                    many.len(),
                    listed.join(", "),
                    if many.len() > 5 { ", …" } else { "" }
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str, group: &str) -> Preset {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:HasSettings="True" crs:Exposure2012="+0.50"/></rdf:RDF></x:xmpmeta>"#;
        let path = format!("/presets/{group}/{name}.xmp");
        Preset {
            name: name.into(),
            group: group.into(),
            ..crate::xmp::parse(std::path::Path::new(&path), xmp).unwrap()
        }
    }
    fn name(name: &str, group: Option<&str>) -> PresetTarget {
        PresetTarget::Name {
            name: name.into(),
            group: group.map(Into::into),
        }
    }

    #[test]
    fn names_resolve_exactly_then_by_a_unique_part() {
        let presets = [
            preset("Ett B&W", "Mine"),
            preset("Ett B&W New", "Mine"),
            preset("Ett B&W New", "Other"),
            preset("Warm", "Mine"),
        ];
        assert_eq!(find(&presets, &name("ett b&w", None)), Ok(0));
        assert_eq!(find(&presets, &name("warm", None)), Ok(3));
        assert_eq!(find(&presets, &name("ar", None)), Ok(3));
        assert_eq!(find(&presets, &name("Ett B&W New", Some("other"))), Ok(2));
        let accented = [preset("Lato", "Été")];
        assert_eq!(find(&accented, &name("lato", Some("été"))), Ok(0));
        let ambiguous = find(&presets, &name("Ett B&W New", None)).unwrap_err();
        assert_eq!(ambiguous.code, "ambiguous");
        assert!(ambiguous.message.contains("Mine / Ett B&W New"));
        assert_eq!(
            find(&presets, &name("Ett", None)).unwrap_err().code,
            "ambiguous"
        );
        assert_eq!(
            find(&presets, &name("Cool", None)).unwrap_err().code,
            "not_found"
        );
        let lone = [preset("Warm", "Mine")];
        assert_eq!(
            find(&lone, &name(" ", None)).unwrap_err().code,
            "invalid_request"
        );
        assert_eq!(
            find(&presets, &name("Warm", Some("Other")))
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[test]
    fn ids_name_only_presets_in_the_library() {
        let presets = [preset("Warm", "Mine")];
        assert_eq!(
            find(&presets, &PresetTarget::Id(presets[0].id.clone())),
            Ok(0)
        );
        // Any other path, even of a readable file, is not looked up.
        let other = PresetTarget::Id("/etc/hosts".into());
        assert_eq!(find(&presets, &other).unwrap_err().code, "not_found");
    }

    #[test]
    fn summaries_carry_the_open_photos_issues_and_filter_by_group() {
        let presets = [preset("Warm", "Mine"), preset("Cool", "Other")];
        let issues = [None, Some("needs a profile".to_string())];
        let all = summaries(&presets, &issues, None);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].issue.as_deref(), Some("needs a profile"));
        let other = summaries(&presets, &[], Some("other"));
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].name, "Cool");
        assert_eq!(other[0].issue, None);
    }
}
