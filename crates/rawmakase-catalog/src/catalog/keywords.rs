//! The catalog's keywords as a whole, as Lightroom's Keyword List shows them:
//! every keyword with its parent and how many photos have it.
use super::db::{Reads, Write, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::metadata::{keyword_key, keyword_name};
use anyhow::{Context, Result, bail, ensure};
use std::collections::{HashMap, HashSet};

/// A keyword of the Keyword List.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedKeyword {
    pub id: i64,
    pub name: String,
    /// `None` at the top level, Lightroom's hidden root's children included.
    pub parent: Option<i64>,
    /// The photos that have this keyword itself, virtual copies counted.
    pub photos: usize,
}

impl Catalog {
    /// Every keyword but Lightroom's hidden root, ordered by id.
    pub fn keyword_list(&self) -> Result<Vec<ListedKeyword>> {
        keyword_list(&self.db)
    }
}

pub(super) fn keyword_list(db: &impl Reads) -> Result<Vec<ListedKeyword>> {
    row! {
        struct Row {
            id: i64,
            name: String,
            parent: Option<i64>,
            photos: i64,
        }
    }
    let rows: Vec<Row> = db.read(
        sql!(
            "SELECT k.id, k.name, k.parent,
                    (SELECT count(*) FROM photo_keywords pk WHERE pk.keyword = k.id)
             FROM keywords k ORDER BY k.id"
        ),
        &[],
    )?;
    let roots: std::collections::HashSet<i64> = rows
        .iter()
        .filter(|r| is_root(&r.name, r.parent))
        .map(|r| r.id)
        .collect();
    Ok(rows
        .into_iter()
        .filter(|r| !roots.contains(&r.id))
        .map(|r| ListedKeyword {
            id: r.id,
            parent: r.parent.filter(|p| !roots.contains(p)),
            name: r.name,
            photos: usize::try_from(r.photos).unwrap_or(0),
        })
        .collect())
}

/// How the Library names each keyword, by id: its path, its own name
/// first, `Paris < France`, as typed. Keywords of one name under different
/// parents are told apart in the Metadata filter, as Lightroom's Keyword
/// column tells them, and a keyword's name there doesn't change when
/// another is made or deleted, so a filter that chose it keeps it.
pub fn keyword_labels(keywords: &[ListedKeyword]) -> HashMap<i64, String> {
    let by_id: HashMap<i64, &ListedKeyword> = keywords.iter().map(|k| (k.id, k)).collect();
    keywords
        .iter()
        .map(|k| {
            let mut names: Vec<&str> = Vec::new();
            let mut next = Some(k.id);
            // A cycle in a damaged catalog would never end.
            while let Some(k) = next.and_then(|n| by_id.get(&n))
                && names.len() < 64
            {
                names.push(&k.name);
                next = k.parent;
            }
            (k.id, names.join(" < "))
        })
        .collect()
}

/// The labels of keywords `ids` (see [`keyword_labels`]), sorted.
pub fn labels_of(ids: &[i64], labels: &HashMap<i64, String>) -> Vec<String> {
    let mut names: Vec<String> = ids
        .iter()
        .filter_map(|id| labels.get(id).cloned())
        .collect();
    names.sort();
    names
}

/// Lightroom's hidden root: a nameless keyword with no parent, which its
/// top-level keywords are under.
pub(super) fn is_root(name: &str, parent: Option<i64>) -> bool {
    name.is_empty() && parent.is_none()
}

/// Some keywords as they are, absent ones included, with their export
/// options and the photos that have them: what [`Catalog::restore_keywords`]
/// puts back. A Keyword List change keeps one from before and one from after.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeywordRows {
    /// Every keyword the rows are of, those absent included.
    ids: Vec<i64>,
    keywords: Vec<KeywordRow>,
    /// The photos with each, by keyword.
    tagged: Vec<(PhotoId, i64)>,
}
impl KeywordRows {
    /// The photos that have one of the keywords.
    pub fn photos(&self) -> impl Iterator<Item = PhotoId> + '_ {
        self.tagged.iter().map(|(photo, _)| *photo)
    }
    /// Leaves photo `id` out, once it is no longer in the catalog: its id may
    /// be given to another photo.
    pub fn forget_photo(&mut self, id: PhotoId) {
        self.tagged.retain(|(photo, _)| *photo != id);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct KeywordRow {
    id: i64,
    name: String,
    /// As stored: Lightroom's hidden root included.
    parent: Option<i64>,
    /// Include on Export and Export Containing Keywords, where a row has them.
    export: Option<(bool, bool)>,
    /// When it was last given to a photo, for Recent Keywords.
    applied: Option<i64>,
}

/// What a Keyword List change did: the keywords it touched before and
/// after, for undo and redo.
#[derive(Clone, Debug, PartialEq)]
pub struct KeywordChange {
    pub before: KeywordRows,
    pub after: KeywordRows,
}

impl Catalog {
    /// Makes keyword `name` under `parent` (`None`: at the top level, under
    /// Lightroom's hidden root where there is one). Refused when its parent
    /// already has a keyword of that name, ignoring case.
    pub fn create_keyword(
        &mut self,
        name: &str,
        parent: Option<i64>,
    ) -> Result<(i64, KeywordChange)> {
        let name = keyword_name(name);
        check_name(&name)?;
        self.db.write(|w| {
            let parent = match parent {
                Some(p) => {
                    ensure!(exists(w, p)?, "That keyword is no longer in the catalog");
                    Some(p)
                }
                None => root(w)?,
            };
            refuse_sibling(w, parent, &name, None)?;
            let id = new_keyword(w, &name, parent)?;
            Ok((
                id,
                KeywordChange {
                    before: KeywordRows {
                        ids: vec![id],
                        ..Default::default()
                    },
                    after: rows_of(w, &[id])?,
                },
            ))
        })
    }
    /// Renames keyword `id`. Refused when another keyword of its parent has
    /// the name, ignoring case: nothing is merged, as in Lightroom.
    pub fn rename_keyword(&mut self, id: i64, name: &str) -> Result<KeywordChange> {
        let name = keyword_name(name);
        check_name(&name)?;
        self.db.write(|w| {
            let parent: Option<Option<i64>> =
                w.read_optional(sql!("SELECT parent FROM keywords WHERE id=?"), &[&id])?;
            let parent = parent.context("That keyword is no longer in the catalog")?;
            ensure!(
                parent.is_some() || root(w)? != Some(id),
                "Lightroom's root keyword can't be renamed"
            );
            refuse_sibling(w, parent, &name, Some(id))?;
            let before = rows_of(w, &[id])?;
            w.execute(sql!("UPDATE keywords SET name=? WHERE id=?"), &[&name, &id])?;
            Ok(KeywordChange {
                before,
                after: rows_of(w, &[id])?,
            })
        })
    }
    /// Deletes keywords `ids` with every keyword inside them, from every
    /// photo too, in one transaction.
    pub fn delete_keywords(&mut self, ids: &[i64]) -> Result<KeywordChange> {
        self.db.write(|w| {
            if let Some(root) = root(w)? {
                ensure!(
                    !ids.contains(&root),
                    "Lightroom's root keyword can't be deleted"
                );
            }
            let all = with_descendants(w, ids)?;
            let before = rows_of(w, &all)?;
            let after = KeywordRows {
                ids: all,
                ..Default::default()
            };
            put(w, &after)?;
            Ok(KeywordChange { before, after })
        })
    }
    /// The keywords no photo has, nor any keyword inside them: what Purge
    /// Unused Keywords deletes.
    pub fn unused_keywords(&self) -> Result<Vec<i64>> {
        let list = keyword_list(&self.db)?;
        let mut used: HashSet<i64> = HashSet::new();
        let parents: HashMap<i64, Option<i64>> = list.iter().map(|k| (k.id, k.parent)).collect();
        for k in list.iter().filter(|k| k.photos > 0) {
            let mut next = Some(k.id);
            while let Some(id) = next {
                if !used.insert(id) {
                    break;
                }
                next = parents.get(&id).copied().flatten();
            }
        }
        Ok(list
            .iter()
            .map(|k| k.id)
            .filter(|id| !used.contains(id))
            .collect())
    }
    /// Puts keywords back as `rows` has them, absent ones removed, in one
    /// transaction. Photos no longer in the catalog are left out.
    pub fn restore_keywords(&mut self, rows: &KeywordRows) -> Result<()> {
        self.db.write(|w| put(w, rows))
    }
}

/// Refuses a name Lightroom refuses for a keyword it creates or renames,
/// with its words.
fn check_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "A name must be provided.");
    for (c, what) in [(',', "commas (,)"), (';', "semicolons (;)")] {
        ensure!(
            !name.contains(c),
            "Keyword tags are not allowed to contain {what}."
        );
    }
    // Typed, they say where a keyword's parents are: a name with one couldn't
    // be typed, nor kept in a keyword set.
    ensure!(
        !name.contains(['<', '>']),
        "Keyword tags are not allowed to contain < or >, which separate a keyword from its parents when typed."
    );
    ensure!(
        !name.contains('|'),
        "Keyword tags and synonyms are not allowed to contain vertical bars (|)."
    );
    ensure!(
        !name.ends_with('*'),
        "Keyword tags and synonyms are not allowed to end with an asterisk (*)."
    );
    Ok(())
}

fn exists(db: &impl Reads, id: i64) -> Result<bool> {
    let found: Option<i64> =
        db.read_optional(sql!("SELECT id FROM keywords WHERE id=?"), &[&id])?;
    Ok(found.is_some())
}

/// Lightroom's hidden root, where the catalog has one.
pub(super) fn root(db: &impl Reads) -> Result<Option<i64>> {
    db.read_optional(
        sql!("SELECT id FROM keywords WHERE name='' AND parent IS NULL ORDER BY id LIMIT 1"),
        &[],
    )
}

/// Refuses `name` under `parent` when a keyword there other than `except`
/// has it, ignoring case.
fn refuse_sibling(
    db: &impl Reads,
    parent: Option<i64>,
    name: &str,
    except: Option<i64>,
) -> Result<()> {
    row! {
        struct Named {
            id: i64,
            name: String,
        }
    }
    // The top level is both beside and under Lightroom's hidden root.
    let top = parent.is_none() || parent == root(db)?;
    let siblings: Vec<Named> = if top {
        db.read(
            sql!(
                "SELECT id, name FROM keywords WHERE (parent IS NULL AND name<>'')
                 OR parent IN (SELECT id FROM keywords WHERE name='' AND parent IS NULL)"
            ),
            &[],
        )?
    } else {
        db.read(
            sql!("SELECT id, name FROM keywords WHERE parent=?"),
            &[&parent],
        )?
    };
    let key = keyword_key(name);
    if let Some(taken) = siblings
        .iter()
        .find(|s| Some(s.id) != except && keyword_key(&s.name) == key)
    {
        bail!(
            "The name “{}” is already taken. Please choose a different name.",
            taken.name
        );
    }
    Ok(())
}

/// Makes keyword `name` under `parent` with an id never given before, so
/// undoing a deletion puts back the keyword deleted, never another made
/// since in its place.
pub(super) fn new_keyword(w: &mut Write<'_>, name: &str, parent: Option<i64>) -> Result<i64> {
    let id = highest_id(w)? + 1;
    w.execute(
        sql!("INSERT INTO keywords(id, name, parent) VALUES (?, ?, ?)"),
        &[&id, &name, &parent],
    )?;
    remember_highest_id(w, id)?;
    Ok(id)
}

/// The highest keyword id the catalog has given: its counter's, else, for a
/// catalog this release never gave one in (made by an earlier release or
/// imported from Lightroom), its highest keyword's.
fn highest_id(w: &mut Write<'_>) -> Result<i64> {
    let last: Option<i64> = w.read_optional(
        sql!("SELECT value FROM catalog_counters WHERE name='keyword_id'"),
        &[],
    )?;
    let highest: Option<i64> =
        w.read_optional(sql!("SELECT COALESCE(max(id), 0) FROM keywords"), &[])?;
    Ok(last.unwrap_or(0).max(highest.unwrap_or(0)))
}

/// Records `id` as the highest keyword id given.
fn remember_highest_id(w: &mut Write<'_>, id: i64) -> Result<()> {
    w.execute(
        sql!(
            "INSERT INTO catalog_counters(name, value) VALUES ('keyword_id', ?)
             ON CONFLICT(name) DO UPDATE SET value=excluded.value"
        ),
        &[&id],
    )?;
    Ok(())
}

/// `ids` and every keyword inside them, each once.
fn with_descendants(db: &impl Reads, ids: &[i64]) -> Result<Vec<i64>> {
    row! {
        struct Link {
            id: i64,
            parent: Option<i64>,
        }
    }
    let links: Vec<Link> = db.read(sql!("SELECT id, parent FROM keywords"), &[])?;
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    for l in &links {
        if let Some(p) = l.parent {
            children.entry(p).or_default().push(l.id);
        }
    }
    let mut all = Vec::new();
    let mut seen = HashSet::new();
    let mut next: Vec<i64> = ids.to_vec();
    while let Some(id) = next.pop() {
        if seen.insert(id) {
            all.push(id);
            next.extend(children.get(&id).into_iter().flatten());
        }
    }
    Ok(all)
}

/// Keywords `ids` as they are now.
fn rows_of(db: &impl Reads, ids: &[i64]) -> Result<KeywordRows> {
    row! {
        struct Stored {
            id: i64,
            name: String,
            parent: Option<i64>,
            include: Option<bool>,
            parents: Option<bool>,
            applied: Option<i64>,
        }
    }
    row! {
        struct Tag {
            photo: PhotoId,
            keyword: i64,
        }
    }
    let mut rows = KeywordRows {
        ids: ids.to_vec(),
        ..Default::default()
    };
    for id in ids {
        if let Some(k) = db.read_optional::<Stored>(
            sql!(
                "SELECT k.id, k.name, k.parent, e.include, e.parents, a.applied
                 FROM keywords k LEFT JOIN keyword_export e ON e.keyword=k.id
                 LEFT JOIN keyword_applied a ON a.keyword=k.id WHERE k.id=?"
            ),
            &[id],
        )? {
            rows.keywords.push(KeywordRow {
                id: k.id,
                name: k.name,
                parent: k.parent,
                export: k.include.zip(k.parents),
                applied: k.applied,
            });
        }
        let tags: Vec<Tag> = db.read(
            sql!("SELECT photo, keyword FROM photo_keywords WHERE keyword=? ORDER BY photo"),
            &[id],
        )?;
        rows.tagged
            .extend(tags.into_iter().map(|t| (t.photo, t.keyword)));
    }
    Ok(rows)
}

/// Writes `rows` over the keywords they are of.
fn put(w: &mut Write<'_>, rows: &KeywordRows) -> Result<()> {
    // Kept before any is deleted: once the highest is gone, its id would be
    // given again, and undoing the deletion would take the new keyword's place.
    let highest = highest_id(w)?;
    remember_highest_id(w, highest)?;
    for id in &rows.ids {
        w.execute(sql!("DELETE FROM photo_keywords WHERE keyword=?"), &[id])?;
        w.execute(sql!("DELETE FROM keyword_export WHERE keyword=?"), &[id])?;
        w.execute(sql!("DELETE FROM keyword_applied WHERE keyword=?"), &[id])?;
        w.execute(sql!("DELETE FROM keywords WHERE id=?"), &[id])?;
    }
    for k in &rows.keywords {
        w.execute(
            sql!("INSERT INTO keywords(id, name, parent) VALUES (?, ?, ?)"),
            &[&k.id, &k.name, &k.parent],
        )?;
        if let Some((include, parents)) = k.export {
            w.execute(
                sql!("INSERT INTO keyword_export(keyword, include, parents) VALUES (?, ?, ?)"),
                &[&k.id, &include, &parents],
            )?;
        }
        if let Some(applied) = k.applied {
            w.execute(
                sql!("INSERT INTO keyword_applied(keyword, applied) VALUES (?, ?)"),
                &[&k.id, &applied],
            )?;
        }
    }
    for (photo, keyword) in &rows.tagged {
        w.execute(
            sql!(
                "INSERT INTO photo_keywords(photo, keyword)
                 SELECT ?, ? WHERE EXISTS (SELECT 1 FROM photos WHERE id=?)
                 ON CONFLICT DO NOTHING"
            ),
            &[photo, keyword, photo],
        )?;
    }
    Ok(())
}

/// The keyword a typed path names (top first), as Lightroom finds it: a
/// single name at any level, ignoring case, preferring one in the same case,
/// then one at the top level, then the first; for a path, its top name at
/// any level, ignoring case, and each name below under the one above it.
/// The names found or `None` from the first level missing, with that level.
fn find_typed(all: &[ListedKeyword], path: &[String]) -> (Option<i64>, usize) {
    fn named<'a>(
        all: &'a [ListedKeyword],
        key: &'a str,
    ) -> impl Iterator<Item = &'a ListedKeyword> + 'a {
        all.iter().filter(move |k| keyword_key(&k.name) == key)
    }
    let name = keyword_name(&path[0]);
    let key = keyword_key(&name);
    let top = if path.len() == 1 {
        named(all, &key)
            .find(|k| k.name == name)
            .or_else(|| named(all, &key).find(|k| k.parent.is_none()))
            .or_else(|| named(all, &key).next())
    } else {
        named(all, &key).next()
    };
    let Some(mut found) = top.map(|k| k.id) else {
        return (None, 0);
    };
    for (level, name) in path.iter().enumerate().skip(1) {
        let key = keyword_key(name);
        match named(all, &key).find(|k| k.parent == Some(found)) {
            Some(k) => found = k.id,
            None => return (Some(found), level),
        }
    }
    (Some(found), path.len())
}

/// The keyword typed at `path` (top first), found as [`find_typed`] does,
/// else made with the names missing, a new top-level one under Lightroom's
/// hidden root where there is one.
pub(super) fn typed_keyword(w: &mut Write<'_>, path: &[String]) -> Result<i64> {
    ensure!(!path.is_empty(), "A keyword needs a name");
    let path: Vec<String> = path.iter().map(|n| keyword_name(n)).collect();
    ensure!(path.iter().all(|n| !n.is_empty()), "A keyword needs a name");
    let all = keyword_list(w)?;
    let (mut found, level) = find_typed(&all, &path);
    // Names come typed, a comma already taken as a separator, or from a
    // keyword set or a test standing in for a file's XMP, where a name may
    // have one ("Washington, D.C."). Lightroom's other rules are for the
    // Keyword List's Create and Rename.
    if found.is_none() {
        found = Some(new_keyword(w, &path[0], root(w)?)?);
    }
    let mut parent = found.unwrap();
    for name in path.iter().skip(level.max(1)) {
        parent = new_keyword(w, name, Some(parent))?;
    }
    Ok(parent)
}

/// The keyword typed at `path` (top first) among `keywords` (as
/// [`Catalog::keyword_list`] lists them), as [`Catalog::add_keywords`] finds
/// it; `None` when it would be made.
pub fn find_typed_keyword(keywords: &[ListedKeyword], path: &[String]) -> Option<i64> {
    if path.is_empty() {
        return None;
    }
    match find_typed(keywords, path) {
        (Some(id), level) if level == path.len() => Some(id),
        _ => None,
    }
}

impl Catalog {
    /// [`find_typed_keyword`] among the catalog's keywords.
    pub fn find_typed_keyword(&self, path: &[String]) -> Result<Option<i64>> {
        Ok(find_typed_keyword(&keyword_list(&self.db)?, path))
    }
}

/// Makes `keyword` the most recently applied.
pub(super) fn stamp_applied(w: &mut Write<'_>, keyword: i64) -> Result<()> {
    let last: Option<i64> = w.read_optional(
        sql!("SELECT value FROM catalog_counters WHERE name='keyword_applied'"),
        &[],
    )?;
    let next = last.unwrap_or(0) + 1;
    w.execute(
        sql!(
            "INSERT INTO catalog_counters(name, value) VALUES ('keyword_applied', ?)
             ON CONFLICT(name) DO UPDATE SET value=excluded.value"
        ),
        &[&next],
    )?;
    w.execute(
        sql!(
            "INSERT INTO keyword_applied(keyword, applied) VALUES (?, ?)
             ON CONFLICT(keyword) DO UPDATE SET applied=excluded.applied"
        ),
        &[&keyword, &next],
    )?;
    Ok(())
}

impl Catalog {
    /// Lightroom's Recent Keywords: the keywords most recently given to a
    /// photo by hand, latest first, then, while fewer than nine were, others
    /// by id, as Lightroom fills them.
    pub fn recent_keywords(&self) -> Result<Vec<i64>> {
        let ids: Vec<i64> = self.db.read(
            sql!(
                "SELECT k.id FROM keywords k LEFT JOIN keyword_applied a ON a.keyword=k.id
                 WHERE NOT (k.name='' AND k.parent IS NULL)
                 ORDER BY a.applied IS NULL, a.applied DESC, k.id LIMIT 9"
            ),
            &[],
        )?;
        Ok(ids)
    }
}
