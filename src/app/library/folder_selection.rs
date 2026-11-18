//! The folders chosen in the Folders panel: one, a Shift-click range of the
//! rows shown, or several Cmd/Ctrl-clicked ones. Each is shown with its
//! subfolders.

/// How a folder row was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Click {
    /// A plain click: that folder alone.
    Replace,
    /// Cmd-click (Ctrl elsewhere): add the folder, or take it out.
    Toggle,
    /// Shift-click: the rows from the anchor to this one.
    Range,
}
impl Click {
    pub(super) fn of(modifiers: eframe::egui::Modifiers) -> Self {
        if modifiers.shift {
            Self::Range
        } else if modifiers.command {
            Self::Toggle
        } else {
            Self::Replace
        }
    }
}

/// Folder tree keys; none is All Photographs (or a collection, which
/// clears them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FolderSelection {
    /// The keys chosen, in display order for a range, else in click order.
    keys: Vec<String>,
    /// Where a Shift-click range starts: the folder last clicked without
    /// Shift. Always one of `keys`, or empty when they are.
    anchor: String,
}
impl FolderSelection {
    pub(super) fn one(key: &str) -> Self {
        if key.is_empty() {
            return Self::default();
        }
        Self {
            keys: vec![key.to_string()],
            anchor: key.to_string(),
        }
    }
    /// Follows a folder renamed or moved from tree key `from` to `to`: its
    /// key and those of the folders in it.
    pub(super) fn rekey(&mut self, from: &str, to: &str) {
        for key in self
            .keys
            .iter_mut()
            .chain(std::iter::once(&mut self.anchor))
        {
            if let Some(rest) = rekeyed(key, from) {
                *key = format!("{to}{rest}");
            }
        }
    }
    /// Gives each key `name` has another name for that name.
    pub(super) fn rename(&mut self, name: impl Fn(&str) -> Option<String>) {
        for key in self
            .keys
            .iter_mut()
            .chain(std::iter::once(&mut self.anchor))
        {
            if let Some(named) = name(key) {
                *key = named;
            }
        }
    }
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    pub(super) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub(super) fn keys(&self) -> &[String] {
        &self.keys
    }
    pub(super) fn contains(&self, key: &str) -> bool {
        self.keys.iter().any(|k| k == key)
    }
    /// The folder the others were chosen from, saved as the session's source;
    /// "" for none.
    pub(super) fn anchor(&self) -> &str {
        &self.anchor
    }
    /// The only folder chosen, if there is exactly one.
    pub(super) fn single(&self) -> Option<&str> {
        match self.keys.as_slice() {
            [key] => Some(key),
            _ => None,
        }
    }
    /// Applies a click on `key`; `rows` are the folder rows shown, in order,
    /// for a Shift-click range.
    pub(super) fn click(&mut self, key: &str, click: Click, rows: &[&str]) {
        match click {
            Click::Replace => *self = Self::one(key),
            Click::Toggle if self.is_empty() => *self = Self::one(key),
            Click::Toggle => {
                if let Some(at) = self.keys.iter().position(|k| k == key) {
                    // The last folder stays: the panel always shows something.
                    if self.keys.len() > 1 {
                        self.keys.remove(at);
                        if self.anchor == key {
                            self.anchor = self.keys.last().cloned().unwrap_or_default();
                        }
                    }
                } else {
                    self.keys.push(key.to_string());
                    self.anchor = key.to_string();
                }
            }
            Click::Range => {
                let from = rows.iter().position(|r| *r == self.anchor);
                let to = rows.iter().position(|r| *r == key);
                match (from, to) {
                    (Some(from), Some(to)) => {
                        self.keys = rows[from.min(to)..=from.max(to)]
                            .iter()
                            .map(|r| r.to_string())
                            .collect();
                    }
                    // No anchor shown to start from: the folder alone.
                    _ => *self = Self::one(key),
                }
            }
        }
    }
}

/// The tree key of logical folder `relative` of `root` ("" for the root).
pub(super) fn tree_key(root: crate::catalog::RootId, relative: &str) -> String {
    match relative {
        "" => format!("root:{root}"),
        relative => format!("root:{root}/{relative}"),
    }
}

/// What follows `from` in tree key `key`, when `key` is `from` or a folder
/// in it.
pub(super) fn rekeyed<'a>(key: &'a str, from: &str) -> Option<&'a str> {
    let rest = key.strip_prefix(from)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROWS: [&str; 5] = ["root:1", "root:1/a", "root:1/b", "root:1/c", "root:2"];

    #[test]
    fn shift_click_selects_the_rows_between_the_anchor_and_the_click() {
        let mut selection = FolderSelection::one("root:1/a");
        selection.click("root:1/c", Click::Range, &ROWS);
        assert_eq!(selection.keys(), ["root:1/a", "root:1/b", "root:1/c"]);
        assert_eq!(selection.anchor(), "root:1/a");
        // Upwards from the same anchor, replacing the range.
        selection.click("root:1", Click::Range, &ROWS);
        assert_eq!(selection.keys(), ["root:1", "root:1/a"]);
        assert_eq!(selection.anchor(), "root:1/a");
        // An anchor no longer shown (its parent folded): the folder alone.
        selection.click("root:2", Click::Range, &["root:1", "root:2"]);
        assert_eq!(selection, FolderSelection::one("root:2"));
        // Nothing selected (All Photographs): the folder alone.
        let mut selection = FolderSelection::default();
        selection.click("root:1/b", Click::Range, &ROWS);
        assert_eq!(selection, FolderSelection::one("root:1/b"));
    }

    #[test]
    fn command_click_adds_and_removes_folders_but_keeps_the_last() {
        let mut selection = FolderSelection::default();
        selection.click("root:1/a", Click::Toggle, &ROWS);
        assert_eq!(selection, FolderSelection::one("root:1/a"));
        selection.click("root:2", Click::Toggle, &ROWS);
        assert_eq!(selection.keys(), ["root:1/a", "root:2"]);
        assert_eq!(selection.anchor(), "root:2");
        assert_eq!(selection.single(), None);
        // Taking the anchor out moves it to a folder still chosen.
        selection.click("root:2", Click::Toggle, &ROWS);
        assert_eq!(selection, FolderSelection::one("root:1/a"));
        selection.click("root:1/a", Click::Toggle, &ROWS);
        assert_eq!(selection, FolderSelection::one("root:1/a"));
        // A Shift-click range starts from the last folder added.
        selection.click("root:1/c", Click::Toggle, &ROWS);
        selection.click("root:2", Click::Range, &ROWS);
        assert_eq!(selection.keys(), ["root:1/c", "root:2"]);
        // A plain click goes back to one folder.
        selection.click("root:1/b", Click::Replace, &ROWS);
        assert_eq!(selection.single(), Some("root:1/b"));
    }
}
