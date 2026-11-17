//! Smart collections in the Library: their members, worked out by the
//! catalog session in the background, reach the panel's counts and, for the
//! smart collection shown, the grid, each time the session has them for the
//! current query revision and day. Until then the smart collection shows the
//! photos it had and says it is updating; Select All and Delete Rejected
//! Photos wait, so they never act on photos the rules no longer match. A
//! smart collection whose rules can't be evaluated lists no photos and says
//! why; one that reads keywords says when the catalog doesn't know its
//! keywords' Lightroom synonyms yet.
use super::Library;
use super::collections::SmartCount;
use crate::catalog::{CollectionId, CollectionKind, SynonymState};
use crate::catalog_session::smart::SmartStatus;
use crate::collection_rules::Date;
use std::collections::HashMap;

/// What the Library took last from the session's smart members.
#[derive(Default)]
pub(super) struct Smart {
    /// What the panel shows of each smart collection.
    pub counts: HashMap<CollectionId, SmartCount>,
    /// The revision and day the counts and the source's members are of.
    taken: Option<(u64, Date)>,
    /// The catalog could not be read for the members.
    failed: Option<String>,
    /// Why each smart collection that lists no photos lists none.
    unsupported: HashMap<CollectionId, String>,
    /// Each smart collection's photos, as taken.
    members: HashMap<CollectionId, std::collections::HashSet<crate::catalog::PhotoId>>,
}

/// What the grid says of the smart collection shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum SmartNote {
    Updating,
    Unsupported(String),
    Failed(String),
    /// Its rules read keywords, whose Lightroom synonyms aren't known.
    SynonymsMissing,
}

impl Library {
    /// The smart collection shown, if one is.
    pub(super) fn shown_smart(&self) -> Option<CollectionId> {
        let id = self.filters.collection?;
        self.session
            .collections()
            .iter()
            .any(|c| c.id == id && c.kind == CollectionKind::Smart)
            .then_some(id)
    }
    /// Takes the session's smart members when it has them for this revision
    /// and day; asks for them otherwise. Called every frame.
    pub(in crate::app) fn poll_smart(&mut self) {
        let smart: Vec<CollectionId> = self
            .session
            .collections()
            .iter()
            .filter(|c| c.kind == CollectionKind::Smart)
            .map(|c| c.id)
            .collect();
        if smart.is_empty() {
            if self.smart.taken.is_some() || !self.smart.counts.is_empty() {
                self.smart = Smart::default();
            }
            return;
        }
        let clock = crate::platform::clock::clock();
        let at = (self.session.query_revision(), clock.today);
        if self.smart.taken == Some(at) {
            return;
        }
        let (counts, members, failed) = match self.session.smart_status(&clock) {
            SmartStatus::None | SmartStatus::Pending => {
                if self.smart.counts.is_empty() {
                    // Nothing known yet: the panel shows them as being worked out.
                    self.smart.counts = smart.iter().map(|id| (*id, SmartCount::Pending)).collect();
                    self.sidebar_trees = None;
                }
                return;
            }
            SmartStatus::Ready(members) => {
                let counts: HashMap<CollectionId, SmartCount> = members
                    .iter()
                    .map(|(id, m)| {
                        let count = m
                            .as_ref()
                            .map_or(SmartCount::Unknown, |m| SmartCount::Photos(m.len()));
                        (*id, count)
                    })
                    .collect();
                (counts, Some(members.clone()), None)
            }
            SmartStatus::Failed(error) => {
                let counts = smart.iter().map(|id| (*id, SmartCount::Unknown)).collect();
                (counts, None, Some(error.to_string()))
            }
        };
        self.smart.counts = counts;
        self.smart.taken = Some(at);
        self.smart.failed = failed;
        self.smart.unsupported = members
            .iter()
            .flatten()
            .filter_map(|(id, m)| Some((*id, m.as_ref().err()?.to_string())))
            .collect();
        self.smart.members = members
            .iter()
            .flatten()
            .filter_map(|(id, m)| Some((*id, m.as_ref().ok()?.clone())))
            .collect();
        self.sidebar_trees = None;
        if let Some(id) = self.shown_smart() {
            // A failed read keeps no members: none is shown rather than stale ones.
            self.filters.members = self.smart.members.get(&id).cloned().unwrap_or_default();
            self.filter();
        }
    }
    /// Whether the smart collection shown has its members for this revision
    /// and day; true when no smart collection is shown.
    pub(in crate::app) fn source_settled(&self) -> bool {
        self.shown_smart().is_none()
            || self.smart.taken
                == Some((
                    self.session.query_revision(),
                    crate::platform::clock::clock().today,
                ))
    }
    /// What the grid says of the smart collection shown, if anything.
    pub(in crate::app) fn smart_note(&self) -> Option<SmartNote> {
        let id = self.shown_smart()?;
        if let Some(error) = &self.smart.failed {
            return Some(SmartNote::Failed(error.clone()));
        }
        if !self.source_settled() {
            return Some(SmartNote::Updating);
        }
        if let Some(why) = self.smart.unsupported.get(&id) {
            return Some(SmartNote::Unsupported(why.clone()));
        }
        let reads_keywords = self
            .session
            .smart_rules_listed()
            .iter()
            .any(|(c, rules)| *c == id && rules.as_ref().is_ok_and(|g| g.reads_keywords()));
        (reads_keywords && self.session.synonym_state() == SynonymState::Missing)
            .then_some(SmartNote::SynonymsMissing)
    }
    /// The panel's count of each smart collection.
    pub(super) fn smart_counts(&self) -> &HashMap<CollectionId, SmartCount> {
        &self.smart.counts
    }
    /// Smart collection `id`'s photos as last taken: what it shows while
    /// they are worked out again, none before.
    pub(super) fn smart_members_of(
        &self,
        id: CollectionId,
    ) -> std::collections::HashSet<crate::catalog::PhotoId> {
        self.smart.members.get(&id).cloned().unwrap_or_default()
    }
    /// After a Lightroom import: says how many smart collections have rules
    /// RAWmakase can't evaluate, which list no photos.
    pub(in crate::app) fn note_unsupported_smart(&mut self) {
        let count = self
            .session
            .smart_rules_listed()
            .iter()
            .filter(|(_, rules)| rules.as_ref().map_or(true, |g| g.unsupported().is_some()))
            .count();
        if count > 0 {
            self.messages.warn(unsupported_line(count));
        }
    }
    /// The dismissal of the synonyms notice: the user says this catalog has
    /// no Lightroom synonyms to read.
    pub(in crate::app) fn dismiss_missing_synonyms(&mut self) {
        if let Err(e) = self.session.dismiss_missing_synonyms() {
            self.messages.warn(format!("Could not save that: {e}"));
        }
    }
}

impl SmartNote {
    /// The line the grid shows.
    pub(in crate::app) fn text(&self) -> String {
        match self {
            Self::Updating => "Updating this smart collection…".into(),
            Self::Unsupported(why) => {
                format!("This smart collection lists no photos: a rule can't be evaluated ({why})")
            }
            Self::Failed(error) => error.clone(),
            Self::SynonymsMissing => {
                "Keyword rules don't see Lightroom's keyword synonyms yet: this catalog doesn't have them"
                    .into()
            }
        }
    }
}

/// The import summary's line for `count` smart collections that list no
/// photos.
pub(in crate::app) fn unsupported_line(count: usize) -> String {
    let (them, have) = if count == 1 {
        ("1 smart collection".to_string(), "has")
    } else {
        (format!("{count} smart collections"), "have")
    };
    format!(
        "{them} {have} rules RAWmakase can't evaluate; they list no photos until those rules are removed"
    )
}
