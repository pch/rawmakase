//! Which files go with a photo: its XMP sidecars, by the rule lookup reads
//! them with (`sidecar::sidecar_candidates`), and the edit files releases
//! before the catalog saved beside it. A companion goes with its photo only
//! while that photo is the one file in its folder that reads it, and only
//! into a folder where nothing else would read it, nor the photo read
//! anything already there.
//!
//! Two comparisons, for two questions. Whether another file reads a
//! companion, or the photo would read one already there, is asked loosely
//! (`sidecar::caseless`), so the answer errs towards leaving a companion in
//! place or refusing. Which files are the photo's companions at all is
//! asked strictly: a name lookup would find (one of its candidate names,
//! in another case or normalisation form), or one the volume itself takes
//! for a candidate (the same file object), never a name only alike in a
//! looser sense.
use super::super::folder_sync::folded;
use super::super::legacy_sidecar::companion_paths;
use super::super::sidecar::{caseless, sidecar_names, sidecar_names_caseless};
use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// Every name a companion of the file named `photo` could have, by `sidecar::caseless`.
fn names_of(photo: &OsStr) -> Vec<String> {
    let mut names = sidecar_names_caseless(Path::new(photo));
    names.extend(
        companion_paths(Path::new(photo))
            .iter()
            .map(|p| caseless(p.as_os_str())),
    );
    names
}

/// A folder's names, each folded once, so what a plan asks of each photo
/// there is looked up rather than asked of every name again: which names
/// are alike (by `folder_sync::folded`, by `sidecar::caseless`) and which
/// would read a file as their companion.
pub(super) struct Listing {
    names: Vec<OsString>,
    folded: HashMap<String, Vec<usize>>,
    caseless: HashMap<String, Vec<usize>>,
    /// By a companion's `caseless` name, the names reading it as their
    /// sidecar (by `sidecar::sidecar_names_caseless`), and those reading it
    /// as their earlier releases' edit file; in listing order.
    sidecar_readers: HashMap<String, Vec<usize>>,
    legacy_readers: HashMap<String, Vec<usize>>,
}

impl Listing {
    pub(super) fn new(names: Vec<OsString>) -> Self {
        let mut listing = Self {
            folded: HashMap::new(),
            caseless: HashMap::new(),
            sidecar_readers: HashMap::new(),
            legacy_readers: HashMap::new(),
            names: Vec::new(),
        };
        for name in names {
            listing.push(name);
        }
        listing
    }

    /// The keys name `name` is indexed by in each of [`Self::keyed`]'s
    /// maps.
    fn keys(name: &OsStr) -> [Vec<String>; 4] {
        let sidecars = match name.to_str() {
            Some(_) => sidecar_names_caseless(Path::new(name)),
            None => Vec::new(),
        };
        let legacy = companion_paths(Path::new(name))
            .iter()
            .map(|p| caseless(p.as_os_str()))
            .collect();
        [vec![folded(name)], vec![caseless(name)], sidecars, legacy]
    }

    fn keyed(&mut self) -> [&mut HashMap<String, Vec<usize>>; 4] {
        [
            &mut self.folded,
            &mut self.caseless,
            &mut self.sidecar_readers,
            &mut self.legacy_readers,
        ]
    }

    /// Indexes name `name` at `i`, keeping each key's places in order.
    fn index(&mut self, i: usize, name: &OsStr) {
        for (map, keys) in self.keyed().into_iter().zip(Self::keys(name)) {
            for key in keys {
                let at = map.entry(key).or_default();
                if let Err(place) = at.binary_search(&i) {
                    at.insert(place, i);
                }
            }
        }
    }

    /// Takes name `name` at `i` out of the indexes.
    fn unindex(&mut self, i: usize, name: &OsStr) {
        for (map, keys) in self.keyed().into_iter().zip(Self::keys(name)) {
            for key in keys {
                if let Some(at) = map.get_mut(&key) {
                    at.retain(|&j| j != i);
                    if at.is_empty() {
                        map.remove(&key);
                    }
                }
            }
        }
    }

    /// Adds `name`, last, as a file named so arriving there adds it.
    pub(super) fn push(&mut self, name: OsString) {
        self.index(self.names.len(), &name);
        self.names.push(name);
    }

    /// Takes out every name exactly `name`, as a file named so leaving
    /// there takes it out. The last name takes the place of each.
    pub(super) fn remove(&mut self, name: &OsStr) {
        let exact = |l: &Self| {
            Self::at(&l.folded, &folded(name))
                .iter()
                .copied()
                .find(|&i| l.names[i] == name)
        };
        while let Some(i) = exact(self) {
            let last = self.names.len() - 1;
            let gone = self.names.swap_remove(i);
            self.unindex(i, &gone);
            if i < last {
                let moved = self.names[i].clone();
                self.unindex(last, &moved);
                self.index(i, &moved);
            }
        }
    }

    /// Takes out the names after the first `len`, as [`Self::push`] added
    /// them.
    pub(super) fn truncate(&mut self, len: usize) {
        while self.names.len() > len {
            let Some(name) = self.names.pop() else { break };
            self.unindex(self.names.len(), &name);
        }
    }

    pub(super) fn names(&self) -> &[OsString] {
        &self.names
    }

    fn at<'a>(map: &'a HashMap<String, Vec<usize>>, key: &str) -> &'a [usize] {
        map.get(key).map_or(&[], Vec::as_slice)
    }

    /// The names there `sidecar::caseless` takes for `name`, in listing order.
    pub(super) fn alike(&self, name: &OsStr) -> impl Iterator<Item = &OsString> {
        Self::at(&self.caseless, &caseless(name))
            .iter()
            .map(|&i| &self.names[i])
    }

    /// The names there that would read the file named `companion` as
    /// theirs (the test helper `readers`), but for those `left_out` says are not there.
    fn readers(&self, companion: &OsStr, left_out: &dyn Fn(usize) -> bool) -> Vec<&OsString> {
        if companion.to_str().is_none() {
            return Vec::new();
        }
        let key = caseless(companion);
        let mut found: Vec<&OsString> = Self::at(&self.sidecar_readers, &key)
            .iter()
            .filter(|&&i| !left_out(i) && self.names[i] != companion)
            .map(|&i| &self.names[i])
            .collect();
        for &i in Self::at(&self.legacy_readers, &key) {
            let name = &self.names[i];
            if !left_out(i) && !found.contains(&name) {
                found.push(name);
            }
        }
        found
    }

    /// The companions of the file named `photo` there (the test helper `of`).
    pub(super) fn companions(
        &self,
        photo: &OsStr,
        same_file: &dyn Fn(&OsStr, &OsStr) -> bool,
    ) -> Companions {
        let mut candidates = sidecar_names(Path::new(photo));
        candidates.extend(
            companion_paths(Path::new(photo))
                .into_iter()
                .map(|p| p.into_os_string()),
        );
        let mut found = BTreeSet::new();
        for c in &candidates {
            found.extend(Self::at(&self.folded, &folded(c)));
            found.extend(
                Self::at(&self.caseless, &caseless(c))
                    .iter()
                    .filter(|&&i| same_file(&self.names[i], c)),
            );
        }
        let mut out = Companions::default();
        for name in found.into_iter().map(|i: usize| &self.names[i]) {
            if name == photo || name.to_str().is_none() {
                continue;
            }
            if self
                .readers(name, &|_| false)
                .iter()
                .all(|r| r.as_os_str() == photo)
            {
                out.carried.push(name.clone());
            } else {
                out.shared.push(name.clone());
            }
        }
        out.carried.sort();
        out.shared.sort();
        out
    }

    /// Why the file named `photo` and its `carried` companions can't arrive
    /// in a folder whose names, once everything arriving there is in, are
    /// these but for those `left_out` says are gone (the test helper
    /// `arrival_conflict`).
    pub(super) fn arrival_conflict(
        &self,
        photo: &OsStr,
        carried: &[OsString],
        left_out: &dyn Fn(usize) -> bool,
    ) -> Option<OsString> {
        // Something already there, or arriving with another photo, the photo
        // would take for its own.
        let read: BTreeSet<usize> = names_of(photo)
            .iter()
            .flat_map(|n| Self::at(&self.caseless, n))
            .copied()
            .collect();
        let theirs = read.into_iter().find(|&i| {
            let name = &self.names[i];
            !left_out(i) && !carried.contains(name) && name != photo && name.to_str().is_some()
        });
        if let Some(i) = theirs {
            return Some(self.names[i].clone());
        }
        // Its own companions, which another file there would read too.
        for companion in carried {
            let others = self
                .readers(companion, left_out)
                .into_iter()
                .find(|r| r.as_os_str() != photo);
            if let Some(other) = others {
                return Some(other.clone());
            }
        }
        None
    }
}

/// The names in `names` that would read the file named `companion` as
/// theirs: its sidecar readers, and the file a legacy edit file is named
/// after.
#[cfg(test)]
pub(super) fn readers(companion: &OsStr, names: &[OsString]) -> Vec<OsString> {
    let listing = Listing::new(names.to_vec());
    let found = listing.readers(companion, &|_| false);
    found.into_iter().cloned().collect()
}

/// Whether the file named `name` is an edit file an earlier release saved
/// for the file named `photo`, which names that file's `Identity` inside.
pub(super) fn is_legacy_edit(photo: &OsStr, name: &OsStr) -> bool {
    name.to_str().is_some()
        && companion_paths(Path::new(photo))
            .iter()
            .any(|p| caseless(p.as_os_str()) == caseless(name))
}

/// A photo's companions in its folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Companions {
    /// Read by the photo alone: they go with it.
    pub carried: Vec<OsString>,
    /// Read by another file there too: they stay.
    pub shared: Vec<OsString>,
}

/// The companions of the file named `photo` in a folder whose listing is
/// `names`: the files named as its companions are, in another case or
/// normalisation form at most, or by a name `same_file` (whether two names
/// there are one file object) says the volume takes for one of them.
#[cfg(test)]
pub(super) fn of(
    photo: &OsStr,
    names: &[OsString],
    same_file: &dyn Fn(&OsStr, &OsStr) -> bool,
) -> Companions {
    Listing::new(names.to_vec()).companions(photo, same_file)
}

/// The name companion `companion` of the file named `photo` takes when the
/// photo is renamed `renamed`, so the same rule finds it: the photo's new
/// name (or, for a RAW's stem sidecar, its new stem) and the companion's
/// own ending, in its own case (`.xmp`, `.XMP`). `None` when `companion`
/// isn't one of `photo`'s names.
pub(super) fn renamed(photo: &OsStr, renamed: &OsStr, companion: &OsStr) -> Option<OsString> {
    let (photo, renamed, companion) = (photo.to_str()?, renamed.to_str()?, companion.to_str()?);
    const ENDINGS: [&str; 3] = [".xmp", ".rawmakase.json", ".rawmakase-local.json"];
    let mut bases = vec![(photo.to_string(), renamed.to_string(), &ENDINGS[..])];
    if crate::storage::is_raw(Path::new(photo)) {
        let stem = |name: &str| {
            Path::new(name)
                .with_extension("")
                .to_str()
                .map(String::from)
        };
        bases.push((stem(photo)?, stem(renamed)?, &ENDINGS[..1]));
    }
    for (old, new, endings) in bases {
        for ending in endings {
            let Some(at) = companion.len().checked_sub(ending.len()) else {
                continue;
            };
            let (Some(head), Some(tail)) = (companion.get(..at), companion.get(at..)) else {
                continue;
            };
            if tail.eq_ignore_ascii_case(ending)
                && caseless(OsStr::new(head)) == caseless(OsStr::new(&old))
            {
                return Some(format!("{new}{tail}").into());
            }
        }
    }
    None
}

/// Why the file named `photo` and its `carried` companions can't arrive in a
/// folder whose names, once everything arriving there is in, will be
/// `projected`: the name of a file there the photo would read as its
/// companion, or of one that would read a companion arriving with it.
#[cfg(test)]
pub(super) fn arrival_conflict(
    photo: &OsStr,
    carried: &[OsString],
    projected: &[OsString],
) -> Option<OsString> {
    Listing::new(projected.to_vec()).arrival_conflict(photo, carried, &|_| false)
}
