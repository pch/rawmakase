//! Whether a name is free in a destination folder: on disk, compared as that
//! folder compares names (probed, never assumed; an aspect not shown to be
//! exact is folded); in the catalog, where every photo of the folder holds
//! its name, offline or not, case-folded; and among the names arriving with
//! the same operation. Nothing is ever replaced, so a name taken is a
//! refusal, decided before anything moves and checked again before each
//! step.
use super::super::sidecar::caseless;
use super::disk::Disk;
use crate::fsops::{Folding, FsError, NameRules};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// How names are told apart in one destination folder.
#[derive(Clone, Copy, Debug)]
pub(super) struct Names(pub NameRules);

impl Names {
    /// `name` as the folder compares it.
    pub(super) fn key(&self, name: &OsStr) -> String {
        use unicode_normalization::UnicodeNormalization;
        let text = name.to_string_lossy();
        let text: String = if self.0.normalization == Folding::Exact {
            text.into_owned()
        } else {
            text.nfc().collect()
        };
        if self.0.case == Folding::Exact {
            text
        } else {
            caseless(text.as_ref())
        }
    }
}

/// What a destination folder holds, read once for a plan or a step.
pub(super) struct Destination<'a> {
    pub dir: &'a Path,
    /// Its exact listing.
    pub names: &'a [OsString],
    pub rules: Names,
    /// The filenames of the catalog's photos there, by `sidecar::caseless`.
    pub catalog: &'a HashSet<String>,
}

/// Why a name can't be used there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Taken {
    /// A file or folder there has it, or one the folder takes for the same.
    OnDisk,
    /// A photo of the catalog is named so there, offline or not.
    InCatalog,
}

impl Destination<'_> {
    /// Whether `name` is taken there, before anything arrives.
    pub(super) fn taken(&self, disk: &dyn Disk, name: &OsStr) -> Option<Taken> {
        let key = self.rules.key(name);
        if self
            .names
            .iter()
            .any(|n| n == name || self.rules.key(n) == key)
        {
            return Some(Taken::OnDisk);
        }
        // Looked up as well: a volume that compares names more loosely
        // than its listing shows finds what is there.
        match disk.entry(&self.dir.join(name)) {
            Err(FsError::NotFound) => {}
            _ => return Some(Taken::OnDisk),
        }
        if self.catalog.contains(&caseless(name)) {
            return Some(Taken::InCatalog);
        }
        None
    }
}
