//! The Metadata filter's counts, as Lightroom's Library Filter › Metadata
//! shows them: how many photos have each value of an attribute. The
//! attributes and each photo's values of them are
//! [`crate::collection_rules`]'s. Camera, lens and the shooting settings come
//! from the catalog's photo info, read for every photo at once when the
//! filter or the search first needs them, and again after a reload or once
//! info read from files is saved.
use super::Library;
use super::sort::natural;
pub(super) use crate::collection_rules::{Attribute, Facets, PhotoFacets};
use std::cmp::Ordering;
use std::collections::HashMap;

/// How two values of `attribute` sort: by duration for shutter speeds
/// (1/4000 sec before 1/250 sec), else as people read them (23 mm before
/// 105 mm).
fn compare(attribute: Attribute, a: &str, b: &str) -> Ordering {
    if attribute == Attribute::ShutterSpeed
        && let (Some(x), Some(y)) = (seconds(a), seconds(b))
    {
        return x.total_cmp(&y);
    }
    natural(a, b).then_with(|| a.cmp(b))
}

/// A shutter speed as [`crate::metadata::PhotoInfo::shutter_text`] writes it,
/// in seconds.
fn seconds(text: &str) -> Option<f64> {
    let number = text.strip_suffix(" sec")?;
    match number.split_once('/') {
        Some((top, bottom)) => Some(top.parse::<f64>().ok()? / bottom.parse::<f64>().ok()?),
        None => number.parse().ok(),
    }
}

/// One column's values with their photo counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Counts {
    /// The photos counted, each once, whatever its values: All's count.
    pub total: usize,
    /// Each value and its photos, in the order the attribute sorts, then
    /// the photos without one.
    pub values: Vec<(Option<String>, usize)>,
}
impl Counts {
    pub(super) fn sorted(
        attribute: Attribute,
        total: usize,
        counts: HashMap<Option<String>, usize>,
    ) -> Self {
        let mut values: Vec<_> = counts.into_iter().collect();
        values.sort_by(|(a, _), (b, _)| match (a, b) {
            (Some(a), Some(b)) => compare(attribute, a, b),
            (a, b) => a.is_none().cmp(&b.is_none()),
        });
        Self { total, values }
    }
}

/// The Library's copy of every photo's facets and the counts last shown.
#[derive(Default)]
pub(super) struct Cache {
    facets: Option<PhotoFacets>,
    /// Photo info was saved since the facets were read: they are read again
    /// once the background read ends, not after each of its batches, as each
    /// read is of the whole catalog.
    stale: bool,
    /// Each column's counts, for the photos shown at that `shown_version`,
    /// while the Metadata menu shows them.
    counts: Option<(u64, Vec<Counts>)>,
}
impl Cache {
    /// `attribute`'s counts as last worked out by
    /// [`Library::update_metadata_counts`].
    pub(super) fn counts(&self, attribute: Attribute) -> Option<&Counts> {
        let at = Attribute::ALL.iter().position(|a| *a == attribute)?;
        self.counts.as_ref().map(|(_, counts)| &counts[at])
    }
}

impl Library {
    /// Reads every photo's facets from the catalog, unless they are read
    /// already. A catalog that can't be read gives none, with a warning: the
    /// filter then counts every photo as unknown until the catalog is read
    /// again or more photo info is saved.
    pub(super) fn load_facets(&mut self) {
        if self.facets.facets.is_some() {
            return;
        }
        let infos = self.session.catalog().photo_infos().unwrap_or_else(|e| {
            self.messages.warn(format!(
                "Photo info could not be read for the Metadata filter: {e}"
            ));
            HashMap::new()
        });
        let facets = infos
            .iter()
            .map(|(id, info)| (*id, Facets::of(info)))
            .collect();
        self.facets.facets = Some(facets);
        self.facets.counts = None;
    }
    /// Forgets the facets read, after the catalog was read again.
    pub(super) fn forget_facets(&mut self) {
        self.facets = Cache::default();
    }
    /// After photo info was saved (`saved`) or the background read ended:
    /// whether the facets were forgotten, to be read again, which they are
    /// once the read ends.
    pub(super) fn facets_after_info_saved(&mut self, saved: bool) -> bool {
        self.facets.stale |= saved && self.facets.facets.is_some();
        if self.facets.stale && !self.session.reading_photo_info() {
            self.forget_facets();
            return true;
        }
        false
    }
    pub(super) fn facets(&self) -> Option<&PhotoFacets> {
        self.facets.facets.as_ref()
    }
    /// Works out how many photos have each value of each attribute, among
    /// those the source and the rest of the filter bar show: as the bar would
    /// be with that column's choice cleared, so its other values stay there to
    /// add. Done again only once the photos shown change.
    pub(super) fn update_metadata_counts(&mut self) {
        self.load_facets();
        let version = self.shown_version;
        if self
            .facets
            .counts
            .as_ref()
            .is_none_or(|(at, _)| *at != version)
        {
            let facets = self.facets.facets.as_ref();
            let counts = Attribute::ALL
                .map(|a| {
                    self.filters.counts(
                        self.session.photos(),
                        |path| self.availability.is_available(path),
                        facets,
                        a,
                    )
                })
                .to_vec();
            self.facets.counts = Some((version, counts));
        }
    }
    #[cfg(test)]
    pub(super) fn metadata_counts(&mut self, attribute: Attribute) -> Vec<(Option<String>, usize)> {
        self.update_metadata_counts();
        self.facets.counts(attribute).unwrap().values.clone()
    }
    #[cfg(test)]
    pub(super) fn metadata_total(&mut self, attribute: Attribute) -> usize {
        self.update_metadata_counts();
        self.facets.counts(attribute).unwrap().total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutter_speeds_sort_by_duration() {
        let mut speeds: Vec<&str> = vec!["2 sec", "1/250 sec", "0.8 sec", "1/4000 sec"];
        speeds.sort_by(|a, b| compare(Attribute::ShutterSpeed, a, b));
        assert_eq!(speeds, ["1/4000 sec", "1/250 sec", "0.8 sec", "2 sec"]);
    }
}
