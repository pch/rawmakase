//! Suggestions in the Keywording panel's field: the catalog's keywords
//! matching the name being typed, at any level, as Lightroom offers them.
use crate::catalog::ListedKeyword;
use crate::metadata::keyword_key;
use std::collections::HashMap;

/// The most suggestions shown at once.
pub(super) const MAX_SUGGESTIONS: usize = 10;

/// A keyword offered for the name being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Suggestion {
    pub id: i64,
    /// The keyword and its parents, as typed: `Child < Parent`.
    pub label: String,
}

/// The entry being typed: the text after the last comma, trimmed.
pub(super) fn current_entry(text: &str) -> &str {
    text.rsplit(',').next().unwrap_or("").trim()
}

/// `text` without its last entry, keeping what was typed before it.
pub(super) fn without_current_entry(text: &str) -> String {
    match text.rfind(',') {
        Some(i) => text[..=i].to_string(),
        None => String::new(),
    }
}

/// The keywords whose name starts with `typed`, ignoring case, then those
/// with a word that does; each group by label. None for an empty entry or
/// one with a hierarchy (`Child < Parent`), which is typed out in full.
pub(super) fn suggestions(keywords: &[ListedKeyword], typed: &str) -> Vec<Suggestion> {
    let typed = keyword_key(typed);
    if typed.is_empty() || typed.contains('<') {
        return Vec::new();
    }
    let by_id: HashMap<i64, &ListedKeyword> = keywords.iter().map(|k| (k.id, k)).collect();
    let mut found: Vec<(bool, Suggestion)> = keywords
        .iter()
        .filter_map(|k| {
            let key = keyword_key(&k.name);
            let starts = key.starts_with(&typed);
            let word = !starts
                && key
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| !w.is_empty() && w.starts_with(&typed));
            (starts || word).then(|| {
                (
                    !starts,
                    Suggestion {
                        id: k.id,
                        label: label(k, &by_id),
                    },
                )
            })
        })
        .collect();
    found.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| keyword_key(&a.1.label).cmp(&keyword_key(&b.1.label)))
            .then(a.1.id.cmp(&b.1.id))
    });
    found.truncate(MAX_SUGGESTIONS);
    found.into_iter().map(|(_, s)| s).collect()
}

/// `Child < Parent < Grandparent`.
fn label(keyword: &ListedKeyword, by_id: &HashMap<i64, &ListedKeyword>) -> String {
    let mut names = vec![keyword.name.as_str()];
    let mut next = keyword.parent;
    // A cycle in a damaged catalog would never end.
    while let Some(parent) = next.and_then(|p| by_id.get(&p))
        && names.len() < 64
    {
        names.push(&parent.name);
        next = parent.parent;
    }
    names.join(" < ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyword(id: i64, name: &str, parent: Option<i64>) -> ListedKeyword {
        ListedKeyword {
            id,
            name: name.into(),
            parent,
            photos: 0,
        }
    }

    #[test]
    fn names_at_any_level_match_ignoring_case_prefixes_first() {
        let list = [
            keyword(1, "Places", None),
            keyword(2, "Paris", Some(1)),
            keyword(3, "Old Paris", None),
            keyword(4, "Party", None),
            keyword(5, "Spain", None),
        ];
        let labels: Vec<String> = suggestions(&list, "par")
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert_eq!(labels, ["Paris < Places", "Party", "Old Paris"]);
        assert!(suggestions(&list, "  ").is_empty());
        assert!(suggestions(&list, "Paris < Pl").is_empty());
    }

    #[test]
    fn duplicates_are_each_offered_with_their_own_id() {
        let list = [
            keyword(1, "Dup", None),
            keyword(2, "A", None),
            keyword(3, "Dup", Some(2)),
        ];
        let found = suggestions(&list, "d");
        assert_eq!(
            found,
            [
                Suggestion {
                    id: 1,
                    label: "Dup".into()
                },
                Suggestion {
                    id: 3,
                    label: "Dup < A".into()
                },
            ]
        );
    }

    #[test]
    fn the_entry_is_what_follows_the_last_comma() {
        assert_eq!(current_entry("Sea, Smith"), "Smith");
        assert_eq!(current_entry(" Sea "), "Sea");
        assert_eq!(without_current_entry("Sea, Smi"), "Sea,");
        assert_eq!(without_current_entry("Smi"), "");
    }
}
