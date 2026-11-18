//! Whether a photo matches: the one owner of the questions the Library's
//! filter bar asks of a photo, and the values they are asked of.
//!
//! A photo is seen as an [`Entry`], a plain borrowed value the app's catalog
//! session assembles from its photo row and photo info, so nothing here knows
//! the catalog, the UI or a file. [`Bar`] is the filter bar's test: a rating
//! compared with [`RatingOp`], flags, [`Label`]s, masters or copies
//! ([`Kind`]), the Metadata filter's chosen values of each [`Attribute`] and
//! the search text. A smart collection's rules are a [`Group`] of [`Rule`]s
//! and nested groups, read from and written as Lightroom's text ([`lua`]) or
//! the stored JSON, and evaluated over the same entries by a [`Matcher`],
//! relative dates taken from an injected [`Clock`].
//!
//! A leaf module: it depends on [`crate::ids`], [`crate::metadata`] and
//! [`crate::storage`] alone.
mod attribute;
mod bar;
mod entry;
mod eval;
pub mod fields;
pub mod lua;
mod rules;

pub use attribute::{Attribute, Facets, PhotoFacets};
pub use bar::{Bar, Kind, LABELS, Label, RatingOp};
pub use entry::{EditFacts, Entry};
pub use eval::{CRITERIA, Clock, Date, Matcher, Unsupported, criteria_name};
pub use rules::{
    Combine, Group, Item, JSON_FORMAT, LIGHTROOM_DEFAULTS, LIGHTROOM_DEFAULTS_SET, Malformed, Rule,
};
