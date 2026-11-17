//! Whether a photo matches: the one owner of the questions the Library's
//! filter bar asks of a photo, and the values they are asked of.
//!
//! A photo is seen as an [`Entry`], a plain borrowed value the app's catalog
//! session assembles from its photo row and photo info, so nothing here knows
//! the catalog, the UI or a file. [`Bar`] is the filter bar's test: a rating
//! compared with [`RatingOp`], flags, [`Label`]s, masters or copies
//! ([`Kind`]), the Metadata filter's chosen values of each [`Attribute`] and
//! the search text. Lightroom's smart collection rules will be evaluated over
//! the same entries.
//!
//! A leaf module: it depends on [`crate::ids`], [`crate::metadata`] and
//! [`crate::storage`] alone.
mod attribute;
mod bar;
mod entry;

pub use attribute::{Attribute, Facets, PhotoFacets};
pub use bar::{Bar, Kind, LABELS, Label, RatingOp};
pub use entry::Entry;
