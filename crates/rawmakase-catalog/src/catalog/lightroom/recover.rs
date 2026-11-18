//! Read Smart Collections from Lightroom Catalog… (issue 998): what an import
//! before this release left behind, read from the user's `.lrcat` into a
//! catalog imported from it earlier. It only fills what is missing: rules of
//! smart collections that have none, the sort of collections that have none,
//! the target collection when none was chosen, and synonyms of keywords that
//! have none. A collection is taken to be Lightroom's only when its id, name,
//! set and kind are the same there; a keyword when its id and name are.
//! Anything else is listed, never changed, and running it again changes
//! nothing. It isn't undoable, as an import isn't.
use super::{ensure_no_live_journal, take_snapshot, verify_snapshot};
use crate::catalog::db::{LightroomWrite, Reads, sql, sqlite_sql};
use crate::catalog::value::row;
use crate::catalog::{Catalog, CollectionId};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// What reading the Lightroom catalog filled in, and what it left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmartRecovery {
    /// Smart collections given their rules.
    pub rules: usize,
    /// Collections given their sort.
    pub sorts: usize,
    /// Whether the target collection was set.
    pub target: bool,
    /// Keywords given their synonyms.
    pub synonyms: usize,
    /// Lightroom's collections with something to read that aren't this
    /// catalog's (another id, name, set or kind here), by name.
    pub collections_apart: Vec<String>,
    /// This catalog's keywords that aren't the Lightroom catalog's (missing
    /// there, or another name), by name; a nameless root keyword as "the
    /// hidden root keyword (id)".
    pub keywords_apart: Vec<String>,
    /// Whether every keyword's synonyms are now known.
    pub synonyms_complete: bool,
}

impl Catalog {
    /// Reads what is missing from Lightroom catalog `source`, from a copy of
    /// it, in one transaction (see the module's documentation).
    pub fn read_smart_collections(&mut self, source: &Path) -> Result<SmartRecovery> {
        ensure_no_live_journal(
            source,
            "Lightroom has this catalog open. Close Lightroom, then read it again",
        )?;
        let (snapshot, _) = take_snapshot(source)?;
        verify_snapshot(snapshot.path())?;
        self.db.with_lightroom(snapshot.path(), recover)
    }
}

fn recover(lr: &mut LightroomWrite<'_>) -> Result<SmartRecovery> {
    let mut out = SmartRecovery::default();
    let matched = collections(lr, &mut out)?;
    if lr.has_table("AgLibraryCollectionContent")? {
        content(lr, &matched, &mut out)?;
    }
    let target_chosen: Option<String> = lr.write().read_optional(
        sql!("SELECT value FROM meta WHERE key='target_collection'"),
        &[],
    )?;
    if target_chosen.is_none() && lr.has_table("Adobe_variablesTable")? {
        let target: Vec<String> = lr.read_sqlite(
            sqlite_sql!(
                "SELECT CAST(value AS TEXT) FROM lr.Adobe_variablesTable
                 WHERE name='targetCollection' AND value IS NOT NULL LIMIT 1"
            ),
            &[],
        )?;
        if let Some(target) = target.first() {
            let quick = target.trim() == "qc";
            let ours = target
                .trim()
                .parse::<f64>()
                .ok()
                .is_some_and(|n| matched.contains_key(&CollectionId(n as i64)));
            if quick || ours {
                crate::catalog::collections::import_target(lr.write(), target)?;
                out.target = lr
                    .write()
                    .read_optional::<String>(
                        sql!("SELECT value FROM meta WHERE key='target_collection'"),
                        &[],
                    )?
                    .is_some();
            }
        }
    }
    synonyms(lr, &mut out)?;
    Ok(out)
}

/// Lightroom's collections that are this catalog's, by id, with their kind.
fn collections(
    lr: &mut LightroomWrite<'_>,
    out: &mut SmartRecovery,
) -> Result<HashMap<CollectionId, String>> {
    row! {
        struct Row {
            id: CollectionId,
            name: String,
            parent: Option<CollectionId>,
            kind: String,
        }
    }
    if !lr.has_table("AgLibraryCollection")? {
        return Ok(HashMap::new());
    }
    let theirs: Vec<Row> = lr.read_sqlite(
        sqlite_sql!(
            "SELECT id_local, COALESCE(name, ''), parent, COALESCE(creationId, '')
             FROM lr.AgLibraryCollection"
        ),
        &[],
    )?;
    let ours: Vec<Row> = lr
        .write()
        .read(sql!("SELECT id, name, parent, kind FROM collections"), &[])?;
    let ours: HashMap<CollectionId, Row> = ours.into_iter().map(|r| (r.id, r)).collect();
    let mut matched = HashMap::new();
    let mut apart = Vec::new();
    for row in theirs {
        match ours.get(&row.id) {
            Some(o) if o.name == row.name && o.parent == row.parent && o.kind == row.kind => {
                matched.insert(row.id, row.kind);
            }
            _ => apart.push((row.id, row.name)),
        }
    }
    // Listed only when they have rules or a sort to read.
    let with_content: HashSet<CollectionId> = if lr.has_table("AgLibraryCollectionContent")? {
        lr.read_sqlite::<CollectionId>(
            sqlite_sql!(
                "SELECT DISTINCT collection FROM lr.AgLibraryCollectionContent
                 WHERE owningModule IN ('ag.library.smart_collection',
                     'com.adobe.ag.library.sortType')"
            ),
            &[],
        )?
        .into_iter()
        .collect()
    } else {
        HashSet::new()
    };
    out.collections_apart = apart
        .into_iter()
        .filter(|(id, _)| with_content.contains(id))
        .map(|(_, name)| name)
        .collect();
    out.collections_apart.sort();
    Ok(matched)
}

/// Rules and sorts of matched collections that have none here.
fn content(
    lr: &mut LightroomWrite<'_>,
    matched: &HashMap<CollectionId, String>,
    out: &mut SmartRecovery,
) -> Result<()> {
    row! {
        struct Content {
            collection: CollectionId,
            module: String,
            content: String,
        }
    }
    let rows: Vec<Content> = lr.read_sqlite(
        sqlite_sql!(
            "SELECT collection, owningModule, content FROM lr.AgLibraryCollectionContent
             WHERE content IS NOT NULL AND owningModule IN ('ag.library.smart_collection',
                 'com.adobe.ag.library.sortType', 'com.adobe.ag.library.sortDirection')
             ORDER BY id_local"
        ),
        &[],
    )?;
    let has_rules: HashSet<CollectionId> = lr
        .write()
        .read::<CollectionId>(sql!("SELECT collection FROM smart_collection_rules"), &[])?
        .into_iter()
        .collect();
    let has_sort: HashSet<CollectionId> = lr
        .write()
        .read::<CollectionId>(sql!("SELECT collection FROM collection_options"), &[])?
        .into_iter()
        .collect();
    let mut sorts: HashMap<CollectionId, (Option<String>, Option<String>)> = HashMap::new();
    let mut ruled = HashSet::new();
    for row in rows {
        let Some(kind) = matched.get(&row.collection) else {
            continue;
        };
        match row.module.as_str() {
            "ag.library.smart_collection"
                if kind == crate::catalog::collections::SMART_KIND
                    && !has_rules.contains(&row.collection)
                    && ruled.insert(row.collection) =>
            {
                let rules = crate::catalog::SmartRules::from_lightroom(&row.content);
                crate::catalog::collections::write_rules(lr.write(), row.collection, &rules)?;
                out.rules += 1;
            }
            "com.adobe.ag.library.sortType" => {
                sorts
                    .entry(row.collection)
                    .or_default()
                    .0
                    .get_or_insert(row.content);
            }
            "com.adobe.ag.library.sortDirection" => {
                sorts
                    .entry(row.collection)
                    .or_default()
                    .1
                    .get_or_insert(row.content);
            }
            _ => {}
        }
    }
    for (collection, (sort, direction)) in sorts {
        let is_set = matched
            .get(&collection)
            .is_some_and(|k| k == "com.adobe.ag.library.group");
        let Some(sort) = sort.filter(|_| !is_set && !has_sort.contains(&collection)) else {
            continue;
        };
        let options = crate::catalog::CollectionOptions {
            sort,
            direction: direction.unwrap_or_else(|| "ascending".into()),
        };
        crate::catalog::collections::write_options(lr.write(), collection, &options)?;
        out.sorts += 1;
    }
    Ok(())
}

/// Synonyms of keywords that are Lightroom's and have none here; the
/// synonyms are complete when every keyword here is Lightroom's.
fn synonyms(lr: &mut LightroomWrite<'_>, out: &mut SmartRecovery) -> Result<()> {
    row! {
        struct Keyword {
            id: i64,
            name: String,
        }
    }
    let ours: Vec<Keyword> = lr
        .write()
        .read(sql!("SELECT id, name FROM keywords ORDER BY id"), &[])?;
    let theirs: HashMap<i64, String> = if lr.has_table("AgLibraryKeyword")? {
        lr.read_sqlite::<Keyword>(
            sqlite_sql!("SELECT id_local, COALESCE(name, '') FROM lr.AgLibraryKeyword"),
            &[],
        )?
        .into_iter()
        .map(|k| (k.id, k.name))
        .collect()
    } else {
        HashMap::new()
    };
    let have: HashSet<i64> = lr
        .write()
        .read::<i64>(sql!("SELECT DISTINCT keyword FROM keyword_synonyms"), &[])?
        .into_iter()
        .collect();
    let lightroom_has = lr.has_table("AgLibraryKeywordSynonym")?;
    for k in &ours {
        if theirs.get(&k.id) != Some(&k.name) {
            out.keywords_apart.push(if k.name.is_empty() {
                // Lightroom's nameless root keyword, never shown otherwise.
                format!("the hidden root keyword ({})", k.id)
            } else {
                k.name.clone()
            });
            continue;
        }
        if have.contains(&k.id) || !lightroom_has {
            continue;
        }
        let added = lr.execute_sqlite(
            sqlite_sql!(
                "INSERT INTO keyword_synonyms(keyword, name)
                 SELECT keyword, name FROM lr.AgLibraryKeywordSynonym
                 WHERE keyword = ? AND trim(COALESCE(name, '')) <> ''
                 ON CONFLICT DO NOTHING"
            ),
            &[&k.id],
        )?;
        if added > 0 {
            out.synonyms += 1;
        }
    }
    out.keywords_apart.sort();
    out.synonyms_complete = ours.iter().all(|k| theirs.get(&k.id) == Some(&k.name));
    if out.synonyms_complete {
        crate::catalog::collections::record_synonyms_complete(lr.write())?;
    }
    Ok(())
}
