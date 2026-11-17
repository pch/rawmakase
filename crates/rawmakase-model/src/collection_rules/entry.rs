//! A photo as the questions in this module see it.
use super::{Date, Facets};
use crate::ids::PhotoId;
use crate::metadata::PhotoInfo;

/// One photo or virtual copy, borrowed from what the catalog session read:
/// each copy is its own entry, with its own rating, flag and label.
/// Building one allocates nothing, so an entry is made for each photo every
/// time a list is filtered.
#[derive(Clone, Copy, Debug, Default)]
pub struct Entry<'a> {
    pub id: PhotoId,
    /// Whether it is a virtual copy.
    pub is_copy: bool,
    /// 1 picked, 0 unflagged, -1 rejected.
    pub flag: i32,
    /// 0 for unrated, else 1 to 5.
    pub rating: i32,
    /// The colour label's name ("Red" … "Purple", or a custom label's);
    /// empty for none.
    pub label: &'a str,
    pub filename: &'a str,
    /// A virtual copy's name; empty for a master.
    pub copy_name: &'a str,
    /// The capture time as the catalog stores it, ISO 8601 without a zone
    /// ("2016-06-29T18:24:27.000"); empty when there is none.
    pub captured: &'a str,
    /// The names of its keywords, each whole.
    pub keywords: &'a [String],
    /// Its photo info's camera, lens and settings; None when they aren't read.
    pub facets: Option<&'a Facets>,

    // What smart collection rules read besides; the filter bar leaves them
    // empty.
    /// The folder's whole path.
    pub folder: &'a str,
    /// The names of the static collections it is in, one per line.
    pub collections: &'a str,
    /// Its keywords with their ancestors and synonyms, one per line.
    pub keyword_terms: &'a str,
    pub title: &'a str,
    pub caption: &'a str,
    pub copyright: &'a str,
    /// Its creators, one per line.
    pub creator: &'a str,
    pub info: Option<&'a PhotoInfo>,
    /// Whether it has a GPS location; None when that isn't known.
    pub gps: Option<bool>,
    /// The local date its edit was last saved; None when it has none.
    pub edited: Option<Date>,
    pub edit: EditFacts,
}

/// What a photo's saved edit does, for the rules that ask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditFacts {
    /// Any setting differs from the photo's defaults.
    pub adjusted: bool,
    pub cropped: bool,
    /// Black & White treatment.
    pub grayscale: bool,
}
