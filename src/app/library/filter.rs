//! What the Library shows: the source (a folder scope or collection), the
//! filter bar, the offline filter and the sort order. What the bar asks of a
//! photo (Lightroom's Attribute and Metadata filters, and the search) is
//! [`crate::collection_rules::Bar`]'s; this keeps what is set in it.
//! Cmd+L turns the bar off and on without losing what is set in it.
use super::facets::{Attribute, Counts, PhotoFacets};
use crate::catalog::{Photo, PhotoId};
use crate::catalog_session::entry;
use crate::collection_rules::Bar;
pub(super) use crate::collection_rules::{Kind, Label, RatingOp};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Filters {
    /// Folder ids in the chosen folder and its subfolders; None is All Photographs.
    pub folder_scope: Option<HashSet<crate::catalog::FolderId>>,
    pub collection: Option<crate::catalog::CollectionId>,
    /// The chosen collection's photos.
    pub members: HashSet<PhotoId>,
    pub query: String,
    /// The flags shown (1 picked, 0 unflagged, -1 rejected); none is every flag.
    pub flags: BTreeSet<i32>,
    /// The rating compared with `rating_op`; None is every rating.
    pub rating: Option<i32>,
    pub rating_op: RatingOp,
    /// The labels shown; none is every label.
    pub labels: BTreeSet<Label>,
    pub kind: Kind,
    /// The Metadata filter: the values chosen for each attribute, None for
    /// photos without one; an attribute left out, or empty, is every value.
    pub metadata: BTreeMap<Attribute, BTreeSet<Option<String>>>,
    /// Cmd+L: whether the bar's settings apply.
    pub enabled: bool,
    pub only_missing: bool,
    /// Previous Import: only the photos the last import added, in `members`.
    pub previous_import: bool,
    pub sort: super::sort::Sort,
    pub reverse: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Self {
            folder_scope: None,
            collection: None,
            members: HashSet::new(),
            query: String::new(),
            flags: BTreeSet::new(),
            rating: None,
            rating_op: RatingOp::default(),
            labels: BTreeSet::new(),
            kind: Kind::default(),
            metadata: BTreeMap::new(),
            enabled: true,
            only_missing: false,
            previous_import: false,
            sort: Default::default(),
            reverse: false,
        }
    }
}
impl Filters {
    /// Indices into `photos` of the ones shown, in display order; `keys`
    /// are what the sort order needs (see [`super::sort::Sort::keys`]), and
    /// `facets` the photos' camera, lens and settings, read while
    /// [`Self::needs_facets`].
    pub(super) fn visible(
        &self,
        photos: &[Photo],
        available: impl Fn(&Path) -> bool,
        keys: &super::sort::Keys,
        facets: Option<&PhotoFacets>,
    ) -> Vec<usize> {
        let query = self.query.to_lowercase();
        let mut visible: Vec<usize> = photos
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.source_holds(p, &available)
                    && (!self.enabled || self.bar(&query).shows(&entry(p, facets), None))
            })
            .map(|(i, _)| i)
            .collect();
        self.sort.sort(photos, &mut visible, keys, self.reverse);
        if self.shows_stacks() {
            visible = stacked(photos, visible);
        }
        visible
    }
    /// Whether the source shows folder stacks: All Photographs and folders,
    /// as Lightroom shows them. A collection and Previous Import list their
    /// photos one by one.
    pub(super) fn shows_stacks(&self) -> bool {
        self.collection.is_none() && !self.previous_import
    }
    /// Whether the source (the folders, the collection, the previous
    /// import or the offline photos shown) has `p`, whatever the filter bar
    /// says; `available` tells whether a file is there.
    pub(super) fn source_holds(&self, p: &Photo, available: impl Fn(&Path) -> bool) -> bool {
        self.folder_scope
            .as_ref()
            .is_none_or(|ids| ids.contains(&p.folder))
            && ((self.collection.is_none() && !self.previous_import)
                || self.members.contains(&p.id))
            && (!self.only_missing || !available(&p.path))
    }
    /// Whether filtering needs the photos' facets: the Metadata filter on
    /// photo info, and the search, which finds camera and lens too.
    pub(super) fn needs_facets(&self) -> bool {
        self.enabled
            && (!self.query.is_empty()
                || self
                    .metadata
                    .iter()
                    .any(|(attribute, chosen)| attribute.needs_info() && !chosen.is_empty()))
    }
    /// How many of the photos in the source, that the rest of the bar
    /// lets through, have each of `attribute`'s values; a photo with
    /// several keywords counts once under each. Counted as if the bar were
    /// on, so the menu shows what choosing a value would show. A value chosen
    /// that no such photo has is listed at 0, so it can be unchosen.
    pub(super) fn counts(
        &self,
        photos: &[Photo],
        available: impl Fn(&Path) -> bool,
        facets: Option<&PhotoFacets>,
        attribute: Attribute,
    ) -> Counts {
        let query = self.query.to_lowercase();
        // Borrowed from the photos and their facets until the end: no
        // allocation per keyword.
        let mut known: HashMap<Cow<'_, str>, usize> = HashMap::new();
        let mut unknown = 0;
        let mut total = 0;
        for p in photos {
            if !self.source_holds(p, &available) {
                continue;
            }
            let entry = entry(p, facets);
            if !self.bar(&query).shows(&entry, Some(attribute)) {
                continue;
            }
            total += 1;
            let mut values = attribute.values(&entry);
            // Two keywords may share a name, under different parents.
            values.sort_unstable();
            values.dedup();
            if values.is_empty() {
                unknown += 1;
            }
            for value in values {
                *known.entry(value).or_default() += 1;
            }
        }
        let mut counts: HashMap<Option<String>, usize> = known
            .into_iter()
            .map(|(value, count)| (Some(value.into_owned()), count))
            .collect();
        if unknown > 0 {
            counts.insert(None, unknown);
        }
        for chosen in self.metadata.get(&attribute).into_iter().flatten() {
            counts.entry(chosen.clone()).or_default();
        }
        Counts::sorted(attribute, total, counts)
    }
    /// The bar's test, with the search `query` in lower case.
    fn bar<'a>(&'a self, query: &'a str) -> Bar<'a> {
        Bar {
            flags: &self.flags,
            rating: self.rating.map(|limit| (self.rating_op, limit)),
            labels: &self.labels,
            kind: self.kind,
            metadata: &self.metadata,
            query,
        }
    }
    /// Whether anything is set in the bar, applied or not.
    pub(super) fn bar_set(&self) -> bool {
        !self.query.is_empty()
            || !self.flags.is_empty()
            || self.rating.is_some()
            || !self.labels.is_empty()
            || self.kind != Kind::All
            || self.metadata.values().any(|v| !v.is_empty())
    }
    /// Filters Off: clears the filter bar, leaving the source as it is.
    pub(super) fn clear_bar(&mut self) {
        self.query.clear();
        self.flags.clear();
        self.rating = None;
        self.rating_op = RatingOp::default();
        self.labels.clear();
        self.kind = Kind::All;
        self.metadata.clear();
        self.enabled = true;
    }
}

/// `visible`, filtered and sorted, with each stack's photos together in
/// stack order, where the first of them shown (its top, unless the filters
/// hide it) sorts; a collapsed stack shows that photo alone. A photo the
/// filters hide stays hidden, stacked or not: a filter shows each match.
fn stacked(photos: &[Photo], visible: Vec<usize>) -> Vec<usize> {
    let key = |i: usize| photos[i].stack.map(|s| (s.id, photos[i].folder));
    let mut members: HashMap<_, Vec<usize>> = HashMap::new();
    for &i in &visible {
        if let Some(k) = key(i) {
            members.entry(k).or_default().push(i);
        }
    }
    if members.is_empty() {
        return visible;
    }
    for m in members.values_mut() {
        m.sort_by_key(|i| photos[*i].stack.map(|s| s.index));
    }
    let mut out = Vec::with_capacity(visible.len());
    for &i in &visible {
        let Some(k) = key(i) else {
            out.push(i);
            continue;
        };
        // The stack's photos come where its first one shown sorts.
        if members.get(&k).is_some_and(|m| m[0] == i) {
            let m = members.remove(&k).unwrap_or_default();
            if photos[i].stack.is_some_and(|s| s.collapsed) {
                out.push(i);
            } else {
                out.extend(m);
            }
        }
    }
    out
}

/// Adds `item` to `set`, or takes it out if it is there.
pub(super) fn toggle<T: Ord>(set: &mut BTreeSet<T>, item: T) {
    if !set.remove(&item) {
        set.insert(item);
    }
}
