//! Smart collections' members. Working them out at 100,000 photos took
//! about 0.3 s (the catalog reads and the evaluation about half each, issue
//! 998's benchmark below), so it runs on a background thread with a catalog
//! connection of its own, never on the UI thread.
//!
//! A job is started for one query revision and one local day, with the rules
//! as the session lists them; the worker reads everything else the rules see
//! from the catalog: the photos, their folders' paths, the static
//! collections they are in, their keywords with ancestors and synonyms, their
//! title, caption, copyright and creators, photo info, and (only when a rule asks) when
//! each edit was saved and what it does. Each photo or virtual copy is one
//! [`Entry`]. A result for an older revision or another day is dropped, and a
//! new job starts; a failed read keeps no members and shows its error until
//! the revision or the day changes.
use super::CatalogSession;
use crate::catalog::{Catalog, CatalogLocation, CollectionId, CollectionKind, Photo, PhotoId};
use crate::collection_rules::{Clock, Date, EditFacts, Entry, Group, Matcher, Unsupported};
use anyhow::Result;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError, channel},
};

/// Each smart collection's photos, or the rule that keeps it from listing
/// any.
pub(crate) type Members = HashMap<CollectionId, Result<HashSet<PhotoId>, Unsupported>>;

/// A keyword's synonyms, by keyword id.
pub(crate) type Synonyms = HashMap<i64, Vec<String>>;

/// What the session knows of smart collections' members now.
#[derive(Debug, PartialEq)]
pub(crate) enum SmartStatus<'a> {
    /// No smart collection, nothing to work out.
    None,
    /// Being worked out for the current revision and day.
    Pending,
    Ready(&'a Members),
    /// The catalog could not be read; no members are known.
    Failed(&'a str),
}

/// The members known, and the job working them out.
#[derive(Default)]
pub(super) struct Smart {
    /// The revision and day of `result`.
    at: Option<(u64, Date)>,
    result: Option<Result<Members, String>>,
    job: Option<Job>,
}

struct Job {
    at: (u64, Date),
    rx: Receiver<Result<Members, String>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl CatalogSession {
    /// The members of the catalog's smart collections at this revision on
    /// `clock`'s day: ready, or pending while a job works them out (started
    /// here when none runs for them). Called again once the session's wake
    /// is called.
    pub(crate) fn smart_status(&mut self, clock: &Clock) -> SmartStatus<'_> {
        let smart = self.smart_rules_listed();
        if smart.is_empty() {
            return SmartStatus::None;
        }
        let at = (self.query_revision, clock.today);
        if let Some(job) = &self.smart.job {
            match job.rx.try_recv() {
                Ok(result) => {
                    let job_at = job.at;
                    self.smart.job = None;
                    if job_at == at {
                        self.smart.at = Some(at);
                        self.smart.result = Some(result);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    // Ended without a result: cancelled, or gone.
                    let (job_at, cancelled) = (job.at, job.cancel.load(Ordering::Relaxed));
                    self.smart.job = None;
                    if job_at == at && !cancelled {
                        self.smart.at = Some(at);
                        self.smart.result =
                            Some(Err("Smart collections stopped unexpectedly".into()));
                    }
                }
                // Working out what no longer applies: it stops at its next
                // check, and the one for now starts once it has.
                Err(TryRecvError::Empty) if job.at != at => {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.smart.at != Some(at) {
            self.smart.result = None;
            // One worker at a time: a superseded one is waited for, never
            // left running beside its replacement.
            if self.smart.job.is_none() {
                self.smart.job = Some(start(
                    self.catalog.location().clone(),
                    smart,
                    *clock,
                    at,
                    self.wake.clone(),
                ));
            }
            return SmartStatus::Pending;
        }
        match &self.smart.result {
            Some(Ok(members)) => SmartStatus::Ready(members),
            Some(Err(error)) => SmartStatus::Failed(error),
            None => SmartStatus::Pending,
        }
    }
}

impl CatalogSession {
    /// Each smart collection listed, with its rules or why they can't be
    /// read.
    pub(crate) fn smart_rules_listed(&self) -> Vec<(CollectionId, Result<Group, Unsupported>)> {
        self.collections
            .iter()
            .filter(|c| c.kind == CollectionKind::Smart)
            .map(|c| {
                let rules = match self.smart_rules.get(&c.id) {
                    Some(rules) => rules.rules().map_err(|e| Unsupported {
                        criteria: String::new(),
                        operation: String::new(),
                        why: format!("Its rules can't be read: {e}"),
                    }),
                    None => Err(Unsupported {
                        criteria: String::new(),
                        operation: String::new(),
                        why: "It has no rules".into(),
                    }),
                };
                (c.id, rules)
            })
            .collect()
    }
}

/// Starts working out `smart`'s members on a thread of its own.
fn start(
    location: CatalogLocation,
    smart: Vec<(CollectionId, Result<Group, Unsupported>)>,
    clock: Clock,
    at: (u64, Date),
    wake: super::Wake,
) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = cancel.clone();
    crate::raw::spawn_background(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let catalog = Catalog::open(location)?;
            let synonyms = catalog.keyword_synonyms()?;
            evaluate(&catalog, &smart, &clock, &synonyms, &cancelled)
        }));
        let result = match result {
            Ok(result) => result.map_err(|e| format!("Smart collections could not be read: {e}")),
            Err(_) => Err("Smart collections stopped unexpectedly".into()),
        };
        if !cancelled.load(Ordering::Relaxed) {
            let _ = tx.send(result);
        }
        // Also when cancelled: the job for now waits for this one to end.
        drop(tx);
        wake();
    });
    Job { at, rx, cancel }
}

/// The members of `smart` collections in `catalog` as it is now, relative
/// dates taken from `clock`. Stops early, with an empty result, once
/// `cancel` is set.
pub(crate) fn evaluate(
    catalog: &Catalog,
    smart: &[(CollectionId, Result<Group, Unsupported>)],
    clock: &Clock,
    synonyms: &Synonyms,
    cancel: &AtomicBool,
) -> Result<Members> {
    let mut members = Members::new();
    let mut matchers: Vec<(CollectionId, Matcher)> = Vec::new();
    for (id, group) in smart {
        match group
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|g| g.compile(clock))
        {
            Ok(matcher) => matchers.push((*id, matcher)),
            Err(why) => {
                members.insert(*id, Err(why));
            }
        }
    }
    if matchers.is_empty() {
        return Ok(members);
    }
    let edit_facts = smart
        .iter()
        .any(|(_, g)| g.as_ref().is_ok_and(Group::needs_edit_facts));
    let Some(inputs) = Inputs::read(catalog, edit_facts, synonyms, cancel)? else {
        return Ok(Members::new());
    };
    // Each photo's collections, in the order of `matchers`.
    let hits: Vec<(PhotoId, Vec<usize>)> = inputs
        .photos
        .par_iter()
        .filter_map(|photo| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let entry = inputs.entry(photo);
            let hits: Vec<usize> = matchers
                .iter()
                .enumerate()
                .filter(|(_, (_, m))| m.matches(&entry))
                .map(|(at, _)| at)
                .collect();
            (!hits.is_empty()).then_some((photo.id, hits))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return Ok(Members::new());
    }
    let mut found: Vec<HashSet<PhotoId>> = vec![HashSet::new(); matchers.len()];
    for (photo, at) in hits {
        for at in at {
            found[at].insert(photo);
        }
    }
    for ((id, _), photos) in matchers.into_iter().zip(found) {
        members.insert(id, Ok(photos));
    }
    Ok(members)
}

/// What the rules read, gathered once.
struct Inputs {
    photos: Vec<Photo>,
    folders: HashMap<crate::catalog::FolderId, String>,
    collections: HashMap<PhotoId, String>,
    keywords: HashMap<PhotoId, String>,
    texts: HashMap<PhotoId, (crate::catalog::RuleTexts, String)>,
    infos: HashMap<PhotoId, crate::metadata::PhotoInfo>,
    edited: HashMap<PhotoId, Date>,
    edits: HashMap<PhotoId, EditFacts>,
}
impl Inputs {
    /// None once `cancel` is set, checked between the reads.
    fn read(
        catalog: &Catalog,
        edit_facts: bool,
        synonyms: &Synonyms,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>> {
        let stop = || cancel.load(Ordering::Relaxed);
        let mut photos = catalog.photos()?;
        if stop() {
            return Ok(None);
        }
        // As the session lists them.
        photos.retain(|p| !crate::storage::is_hidden(Path::new(&p.filename)));
        let folders = catalog
            .folders()?
            .into_iter()
            .map(|f| (f.id, f.path.to_string_lossy().into_owned()))
            .collect();
        // Static collections' names, never smart ones', as Lightroom matches.
        let names: HashMap<CollectionId, String> = catalog
            .collections()?
            .into_iter()
            .filter(|c| c.kind == CollectionKind::Collection)
            .map(|c| (c.id, c.name))
            .collect();
        let mut collections: HashMap<PhotoId, String> = HashMap::new();
        for (collection, members) in catalog.collection_photos()? {
            let Some(name) = names.get(&collection) else {
                continue;
            };
            for photo in members {
                let text = collections.entry(photo).or_default();
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(name);
            }
        }
        if stop() {
            return Ok(None);
        }
        let keywords = keyword_terms(
            &catalog.keyword_tree()?,
            &catalog.photo_keyword_ids()?,
            synonyms,
        );
        if stop() {
            return Ok(None);
        }
        let texts = catalog
            .rule_texts()?
            .into_iter()
            .map(|(id, t)| {
                let creators = t.creators.join("\n");
                (id, (t, creators))
            })
            .collect();
        if stop() {
            return Ok(None);
        }
        let infos = catalog.photo_infos()?;
        if stop() {
            return Ok(None);
        }
        let (edited, edits) = if edit_facts {
            let edited = catalog
                .edit_times()?
                .into_iter()
                .filter_map(|(id, utc)| Some((id, local_date_of(&utc)?)))
                .collect();
            (edited, catalog.edit_facts()?)
        } else {
            Default::default()
        };
        Ok(Some(Self {
            photos,
            folders,
            collections,
            keywords,
            texts,
            infos,
            edited,
            edits,
        }))
    }
    fn entry<'a>(&'a self, p: &'a Photo) -> Entry<'a> {
        let text = self.texts.get(&p.id);
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
            facets: None,
            folder: self.folders.get(&p.folder).map_or("", String::as_str),
            collections: self.collections.get(&p.id).map_or("", String::as_str),
            keyword_terms: self.keywords.get(&p.id).map_or("", String::as_str),
            title: text.map_or("", |(t, _)| t.title.as_str()),
            caption: text.map_or("", |(t, _)| t.caption.as_str()),
            copyright: text.map_or("", |(t, _)| t.copyright.as_str()),
            creator: text.map_or("", |(_, creators)| creators.as_str()),
            info: self.infos.get(&p.id),
            gps: text.and_then(|(t, _)| t.gps),
            edited: self.edited.get(&p.id).copied(),
            edit: self.edits.get(&p.id).copied().unwrap_or_default(),
        }
    }
}

/// Each photo's keywords with their ancestors and synonyms, one per line:
/// Lightroom's Keywords rules match all of them, keywords left out of export
/// included.
fn keyword_terms(
    tree: &[crate::catalog::KeywordNode],
    tags: &[(PhotoId, i64)],
    synonyms: &Synonyms,
) -> HashMap<PhotoId, String> {
    let nodes: HashMap<i64, &crate::catalog::KeywordNode> =
        tree.iter().map(|k| (k.id, k)).collect();
    // Each keyword's terms, worked out once however many photos have it.
    let mut terms_of: HashMap<i64, Vec<&str>> = HashMap::new();
    let mut out: HashMap<PhotoId, Vec<&str>> = HashMap::new();
    for (photo, keyword) in tags {
        let terms = terms_of.entry(*keyword).or_insert_with(|| {
            let mut terms = Vec::new();
            let mut at = Some(*keyword);
            // Bounded by the tree's size, against a cycle in a damaged catalog.
            for _ in 0..=nodes.len() {
                let Some(node) = at.and_then(|id| nodes.get(&id)) else {
                    break;
                };
                if !node.name.trim().is_empty() {
                    terms.push(node.name.as_str());
                }
                terms.extend(
                    synonyms
                        .get(&node.id)
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                );
                at = node.parent;
            }
            terms
        });
        out.entry(*photo).or_default().extend(terms.iter().copied());
    }
    out.into_iter()
        .map(|(photo, mut terms)| {
            terms.sort_unstable();
            terms.dedup();
            (photo, terms.join("\n"))
        })
        .collect()
}

/// The local date of a time the catalog keeps in UTC, "YYYY-MM-DD HH:MM:SS".
fn local_date_of(utc: &str) -> Option<Date> {
    let date = Date::parse(utc)?;
    let time = utc.get(11..19)?;
    let mut parts = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, s) = (parts.next()??, parts.next()??, parts.next()??);
    let seconds = date.0 * 86_400 + h * 3600 + m * 60 + s;
    crate::platform::clock::local_date(seconds).or(Some(date))
}

#[cfg(test)]
#[path = "smart_tests.rs"]
mod tests;
