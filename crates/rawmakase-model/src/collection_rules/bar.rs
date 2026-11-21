//! The Library filter bar's test of an entry. It follows Lightroom's
//! Attribute filter (any combination of flags, a rating compared with ≥, ≤
//! or =, any set of color labels, masters or virtual copies), its Metadata
//! filter (any of each column's chosen values, every column chosen in) and the
//! search.
use super::{Attribute, Entry};
use std::collections::{BTreeMap, BTreeSet};

/// The five color labels, in Lightroom's order.
pub const LABELS: [&str; 5] = ["Red", "Yellow", "Green", "Blue", "Purple"];

/// How the filter bar compares a rating with its limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RatingOp {
    #[default]
    AtLeast,
    AtMost,
    Exactly,
}
impl RatingOp {
    pub const ALL: [Self; 3] = [Self::AtLeast, Self::AtMost, Self::Exactly];
    pub fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => "≥",
            Self::AtMost => "≤",
            Self::Exactly => "=",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::AtLeast => "Rating is greater than or equal to",
            Self::AtMost => "Rating is less than or equal to",
            Self::Exactly => "Rating is equal to",
        }
    }
    pub fn holds(self, rating: i32, limit: i32) -> bool {
        match self {
            Self::AtLeast => rating >= limit,
            Self::AtMost => rating <= limit,
            Self::Exactly => rating == limit,
        }
    }
}

/// A color label the bar can match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Label {
    Color(String),
    /// No label at all.
    None,
    /// Any label other than the five colors, such as a custom label set's.
    Other,
}
impl Label {
    pub fn matches(&self, label: &str) -> bool {
        match self {
            Self::Color(name) => label == name,
            Self::None => label.is_empty(),
            Self::Other => !label.is_empty() && !LABELS.contains(&label),
        }
    }
}

/// Masters, virtual copies or both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    All,
    Masters,
    Copies,
}

/// What the filter bar has set, borrowed from it, as one test of an entry.
#[derive(Clone, Copy, Debug)]
pub struct Bar<'a> {
    /// The flags shown (1 picked, 0 unflagged, -1 rejected); none is every flag.
    pub flags: &'a BTreeSet<i32>,
    /// The rating limit and how it is compared; None is every rating.
    pub rating: Option<(RatingOp, i32)>,
    /// The labels shown; none is every label.
    pub labels: &'a BTreeSet<Label>,
    pub kind: Kind,
    /// The Metadata filter: the values chosen for each attribute, None for
    /// entries without one; an attribute left out, or empty, is every value.
    pub metadata: &'a BTreeMap<Attribute, BTreeSet<Option<String>>>,
    /// The search text, in lower case; empty is no search.
    pub query: &'a str,
}
impl Bar<'_> {
    /// Whether the bar lets `entry` through, `except` a Metadata attribute
    /// left out for its counts.
    pub fn shows(&self, entry: &Entry<'_>, except: Option<Attribute>) -> bool {
        let one = |attribute: Attribute| attribute.values(entry).into_iter().next();
        (self.flags.is_empty() || self.flags.contains(&entry.flag))
            && self
                .rating
                .is_none_or(|(op, limit)| op.holds(entry.rating, limit))
            && (self.labels.is_empty() || self.labels.iter().any(|l| l.matches(entry.label)))
            && match self.kind {
                Kind::All => true,
                Kind::Masters => !entry.is_copy,
                Kind::Copies => entry.is_copy,
            }
            && self.metadata.iter().all(|(attribute, chosen)| {
                if Some(*attribute) == except || chosen.is_empty() {
                    return true;
                }
                let values = attribute.values(entry);
                chosen.iter().any(|c| match c {
                    Some(c) => values.iter().any(|v| v == c),
                    None => values.is_empty(),
                })
            })
            && (self.query.is_empty()
                || format!(
                    "{} {} {} {} {} {} {}",
                    entry.filename,
                    entry.copy_name,
                    entry.keywords.join(", "),
                    entry.captured,
                    entry.label,
                    one(Attribute::Camera).unwrap_or_default(),
                    one(Attribute::Lens).unwrap_or_default(),
                )
                .to_lowercase()
                .contains(self.query))
    }
}
