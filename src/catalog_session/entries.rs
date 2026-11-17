//! The session's photos as [`crate::collection_rules::Entry`] values, the
//! one form the filter bar's tests (and later smart collection rules) read a
//! photo in.
use crate::catalog::Photo;
use crate::collection_rules::{Entry, Facets, PhotoFacets};

/// `p` as an entry, with its photo info's `facets` when they are read.
pub(crate) fn entry<'a>(p: &'a Photo, facets: Option<&'a PhotoFacets>) -> Entry<'a> {
    Entry {
        id: p.id,
        is_copy: p.master.is_some(),
        flag: p.flag,
        rating: p.rating,
        label: &p.label,
        filename: &p.filename,
        copy_name: &p.copy_name,
        captured: &p.captured,
        keywords: &p.keywords,
        facets: facets.and_then(|f| facets_of(f, p)),
        ..Entry::default()
    }
}

/// `p`'s facets: its own, else its master's, for a virtual copy made since
/// they were read.
fn facets_of<'a>(facets: &'a PhotoFacets, p: &Photo) -> Option<&'a Facets> {
    facets
        .get(&p.id)
        .or_else(|| p.master.and_then(|m| facets.get(&m)))
}
