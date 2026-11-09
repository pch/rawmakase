//! What the Library shows: the source (a folder scope or collection), the
//! filter bar, the offline filter and the sort order. The bar follows
//! Lightroom's Attribute filter: any combination of flags, a rating compared
//! with ≥, ≤ or =, any set of color labels, and masters or virtual copies.
//! Its Metadata menu follows Lightroom's Metadata filter: a column per
//! attribute (camera, lens, settings, file type, keyword), any of a column's
//! values, and every column chosen in.
//! Cmd+L turns the bar off and on without losing what is set in it.
use super::facets::{Attribute, Counts, PhotoFacets, facets_of};
use crate::app::library::photo_metadata::LABELS;
use crate::catalog::{Photo, PhotoId};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RatingOp {
    #[default]
    AtLeast,
    AtMost,
    Exactly,
}
impl RatingOp {
    pub(super) const ALL: [Self; 3] = [Self::AtLeast, Self::AtMost, Self::Exactly];
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => "≥",
            Self::AtMost => "≤",
            Self::Exactly => "=",
        }
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::AtLeast => "Rating is greater than or equal to",
            Self::AtMost => "Rating is less than or equal to",
            Self::Exactly => "Rating is equal to",
        }
    }
    fn holds(self, rating: i32, limit: i32) -> bool {
        match self {
            Self::AtLeast => rating >= limit,
            Self::AtMost => rating <= limit,
            Self::Exactly => rating == limit,
        }
    }
}

/// A color label the bar can match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Label {
    Color(String),
    /// No label at all.
    None,
    /// Any label other than the five colors, such as a custom label set's.
    Other,
}
impl Label {
    fn matches(&self, label: &str) -> bool {
        match self {
            Self::Color(name) => label == name,
            Self::None => label.is_empty(),
            Self::Other => !label.is_empty() && !LABELS.contains(&label),
        }
    }
}

/// Masters, virtual copies or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Kind {
    #[default]
    All,
    Masters,
    Copies,
}

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
                    && (!self.enabled || self.bar_shows(p, &query, facets, None))
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
            if !self.source_holds(p, &available)
                || !self.bar_shows(p, &query, facets, Some(attribute))
            {
                continue;
            }
            total += 1;
            let mut values = attribute.values(p, facets.and_then(|f| facets_of(f, p)));
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
    /// Whether the filter bar lets `p` through, `except` a Metadata
    /// attribute left out for its counts; `query` is in lower case.
    fn bar_shows(
        &self,
        p: &Photo,
        query: &str,
        facets: Option<&PhotoFacets>,
        except: Option<Attribute>,
    ) -> bool {
        let facets = facets.and_then(|f| facets_of(f, p));
        let one = |attribute: Attribute| attribute.values(p, facets).into_iter().next();
        (self.flags.is_empty() || self.flags.contains(&p.flag))
            && self
                .rating
                .is_none_or(|limit| self.rating_op.holds(p.rating, limit))
            && (self.labels.is_empty() || self.labels.iter().any(|l| l.matches(&p.label)))
            && match self.kind {
                Kind::All => true,
                Kind::Masters => p.master.is_none(),
                Kind::Copies => p.master.is_some(),
            }
            && self.metadata.iter().all(|(attribute, chosen)| {
                if Some(*attribute) == except || chosen.is_empty() {
                    return true;
                }
                let values = attribute.values(p, facets);
                chosen.iter().any(|c| match c {
                    Some(c) => values.iter().any(|v| v == c),
                    None => values.is_empty(),
                })
            })
            && (query.is_empty()
                || format!(
                    "{} {} {} {} {} {} {}",
                    p.filename,
                    p.copy_name,
                    p.keywords.join(", "),
                    p.captured,
                    p.label,
                    one(Attribute::Camera).unwrap_or_default(),
                    one(Attribute::Lens).unwrap_or_default(),
                )
                .to_lowercase()
                .contains(query))
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
