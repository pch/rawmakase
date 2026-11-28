//! Descriptive metadata: title, caption, creator, copyright, capture time,
//! location and keywords, as set in RAWmakase or imported. Each is kept per
//! photo, a virtual copy's apart from its master's. A field with no row is the
//! file's own (its EXIF, at export); a cleared one is empty whatever the file
//! says.
use super::db::{Reads, Sql, Write, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::metadata::{
    Capture, DEFAULT_LANG, Descriptive, Keyword, LangAlt, Location, TextField, Value, keyword_name,
};
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;

/// The column value a text field is stored under.
fn key(field: TextField) -> &'static str {
    match field {
        TextField::Title => "title",
        TextField::Caption => "caption",
        TextField::Copyright => "copyright",
    }
}
const CREATOR: &str = "creator";

/// A photo's descriptive metadata and keywords as they were, absent rows
/// included, to put back on undo.
#[derive(Clone, Debug, PartialEq)]
pub struct MetadataSnapshot {
    pub photo: PhotoId,
    pub descriptive: Descriptive,
    pub keywords: Vec<i64>,
    /// The capture time it sorts by (`photos.captured`), which a capture
    /// override changes.
    pub captured: String,
}

impl Catalog {
    /// A photo's descriptive overrides.
    pub fn descriptive(&self, id: PhotoId) -> Result<Descriptive> {
        read(&self.db, id)
    }
    /// The overrides of several photos, by photo.
    pub fn descriptive_of(&self, ids: &[PhotoId]) -> Result<HashMap<PhotoId, Descriptive>> {
        ids.iter()
            .map(|id| Ok((*id, read(&self.db, *id)?)))
            .collect()
    }
    /// Replaces the default language of a field on every photo given, keeping
    /// its other languages; empty text clears the field, every language.
    /// One transaction.
    pub fn set_text(&mut self, ids: &[PhotoId], field: TextField, text: &str) -> Result<()> {
        self.update_descriptive(ids, |d| {
            let slot = d.text_mut(field);
            *slot = Some(if text.is_empty() {
                Value::Cleared
            } else {
                let mut langs = match slot.take() {
                    Some(Value::Set(langs)) => langs,
                    _ => LangAlt::default(),
                };
                langs.0.retain(|(lang, _)| lang != DEFAULT_LANG);
                langs.0.insert(0, (DEFAULT_LANG.into(), text.into()));
                Value::Set(langs)
            });
        })
    }
    /// Sets the creators of every photo given, in order; none clears the field.
    pub fn set_creators(&mut self, ids: &[PhotoId], names: &[String]) -> Result<()> {
        let names: Vec<String> = names
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        self.update_descriptive(ids, |d| {
            d.creator = Some(if names.is_empty() {
                Value::Cleared
            } else {
                Value::Set(names.clone())
            });
        })
    }
    /// Gives each photo of `locations` that location where the catalog has
    /// none for it (neither one given nor one removed): Read Metadata from
    /// Files' location from a camera's EXIF. One transaction.
    pub fn fill_locations(&mut self, locations: &[(PhotoId, Location)]) -> Result<()> {
        self.db.write(|w| {
            for (id, location) in locations {
                let mut d = read(w, *id)?;
                if d.location.is_none() {
                    d.location = Some(location.clone());
                    write(w, *id, &d)?;
                }
            }
            Ok(())
        })
    }
    /// Leaves the location of every photo given out, its file's included.
    pub fn clear_location(&mut self, ids: &[PhotoId]) -> Result<()> {
        self.update_descriptive(ids, |d| d.location = Some(Location::Cleared))
    }
    /// Records a capture time from elsewhere than the file, and sorts the
    /// photo by it.
    #[cfg(test)]
    pub fn set_capture(&mut self, id: PhotoId, capture: &Capture) -> Result<()> {
        self.update_descriptive(&[id], |d| d.capture = Some(capture.clone()))
    }
    /// Reads, changes and writes back the overrides of every photo given, in
    /// one transaction.
    fn update_descriptive(
        &mut self,
        ids: &[PhotoId],
        mut f: impl FnMut(&mut Descriptive),
    ) -> Result<()> {
        self.update_descriptive_where(ids, |_, d| {
            f(d);
            true
        })
    }
    /// `update_descriptive`, given each photo's id, writing back only the
    /// photos `f` returns true for.
    pub(super) fn update_descriptive_where(
        &mut self,
        ids: &[PhotoId],
        mut f: impl FnMut(PhotoId, &mut Descriptive) -> bool,
    ) -> Result<()> {
        self.db.write(|w| {
            for &id in ids {
                let mut d = read(w, id)?;
                if f(id, &mut d) {
                    write(w, id, &d)?;
                }
            }
            Ok(())
        })
    }
    /// What `restore_metadata` needs to put these photos back as they are.
    pub fn metadata_snapshot(&self, ids: &[PhotoId]) -> Result<Vec<MetadataSnapshot>> {
        ids.iter()
            .map(|id| {
                Ok(MetadataSnapshot {
                    photo: *id,
                    descriptive: read(&self.db, *id)?,
                    keywords: self.db.read(
                        sql!("SELECT keyword FROM photo_keywords WHERE photo=?"),
                        &[id],
                    )?,
                    captured: self
                        .db
                        .read_optional(sql!("SELECT captured FROM photos WHERE id=?"), &[id])?
                        .context("Unknown photo")?,
                })
            })
            .collect()
    }
    /// Puts photos' descriptive metadata and keywords back as snapshotted,
    /// rows that were absent included, in one transaction.
    pub fn restore_metadata(&mut self, snapshots: &[MetadataSnapshot]) -> Result<()> {
        self.db.write(|w| {
            for s in snapshots {
                // The time it sorts by goes back only with its capture override:
                // one filled in from the file since stays.
                let capture_changed = read(w, s.photo)?.capture != s.descriptive.capture;
                write(w, s.photo, &s.descriptive)?;
                if capture_changed {
                    w.execute(
                        sql!("UPDATE photos SET captured=? WHERE id=?"),
                        &[&s.captured, &s.photo],
                    )?;
                }
                w.execute(
                    sql!("DELETE FROM photo_keywords WHERE photo=?"),
                    &[&s.photo],
                )?;
                for keyword in &s.keywords {
                    tag_photo(w, s.photo, *keyword)?;
                }
            }
            Ok(())
        })
    }
    /// A photo's keywords, each with its path from the top, by path.
    pub fn keywords(&self, id: PhotoId) -> Result<Vec<Keyword>> {
        keywords(&self.db, id)
    }
    /// The keyword at `path` (top first), made where it or its parents are
    /// missing. Names are compared in NFC, case kept.
    #[cfg(any(test, feature = "test-support"))]
    pub fn keyword_at(&mut self, path: &[String]) -> Result<i64> {
        ensure!(!path.is_empty(), "A keyword needs a name");
        self.db.write(|w| keyword_at(w, path))
    }
    /// Adds keywords typed by path (top first) to every photo given, found
    /// as Lightroom finds a typed name (see `keywords::find_typed`) and made
    /// where missing; one transaction.
    pub fn add_keywords(&mut self, ids: &[PhotoId], paths: &[Vec<String>]) -> Result<()> {
        self.db.write(|w| {
            for path in paths {
                let keyword = super::keywords::typed_keyword(w, path)?;
                apply(w, ids, keyword)?;
            }
            Ok(())
        })
    }
    /// Adds keyword `keyword` itself to every photo given, without looking
    /// its name up again; one transaction.
    pub fn add_keyword(&mut self, ids: &[PhotoId], keyword: i64) -> Result<()> {
        self.db.write(|w| {
            let known: Option<i64> =
                w.read_optional(sql!("SELECT id FROM keywords WHERE id=?"), &[&keyword])?;
            ensure!(known.is_some(), "That keyword is no longer in the catalog");
            apply(w, ids, keyword)
        })
    }
    /// Removes a keyword from every photo given that has it.
    pub fn remove_keyword(&mut self, ids: &[PhotoId], keyword: i64) -> Result<()> {
        self.db.write(|w| {
            for id in ids {
                w.execute(
                    sql!("DELETE FROM photo_keywords WHERE photo=? AND keyword=?"),
                    &[id, &keyword],
                )?;
            }
            Ok(())
        })
    }
}

/// A photo's keywords, each with its path from the top, by path.
pub(super) fn keywords(db: &impl Reads, id: PhotoId) -> Result<Vec<Keyword>> {
    let ids: Vec<i64> = db.read(
        sql!("SELECT keyword FROM photo_keywords WHERE photo=?"),
        &[&id],
    )?;
    let mut keywords = ids
        .into_iter()
        .map(|k| keyword(db, k))
        .collect::<Result<Vec<_>>>()?;
    // Lightroom's root, were a photo given it.
    keywords.retain(|k| !k.path.is_empty());
    keywords.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
    Ok(keywords)
}

/// What Import Photos' Apply During Import gives a photo it adds, in the
/// transaction that adds it: `creator` and `copyright` replace what its
/// files say (the camera's, a sidecar's), and `keywords` (paths, top first)
/// are added to its own, as Lightroom applies them (measured in Lightroom
/// Classic 2026). An empty field gives nothing.
pub(super) fn apply_import(
    w: &mut Write<'_>,
    photo: PhotoId,
    creator: &str,
    copyright: &str,
    keywords: &[Vec<String>],
) -> Result<()> {
    let (creator, copyright) = (creator.trim(), copyright.trim());
    if !creator.is_empty() || !copyright.is_empty() {
        let mut d = read(w, photo)?;
        if !creator.is_empty() {
            d.creator = Some(Value::Set(vec![creator.to_string()]));
        }
        if !copyright.is_empty() {
            let slot = d.text_mut(TextField::Copyright);
            let mut langs = match slot.take() {
                Some(Value::Set(langs)) => langs,
                _ => LangAlt::default(),
            };
            langs.0.retain(|(lang, _)| lang != DEFAULT_LANG);
            langs.0.insert(0, (DEFAULT_LANG.into(), copyright.into()));
            *slot = Some(Value::Set(langs));
        }
        write(w, photo, &d)?;
    }
    for path in keywords {
        let keyword = super::keywords::typed_keyword(w, path)?;
        apply(w, &[photo], keyword)?;
    }
    Ok(())
}

/// Gives `photo` `keyword`, unless it has it already; whether it did.
pub(super) fn tag_photo(w: &mut Write<'_>, photo: PhotoId, keyword: i64) -> Result<bool> {
    Ok(w.execute(
        sql!("INSERT INTO photo_keywords(photo, keyword) VALUES (?, ?) ON CONFLICT DO NOTHING"),
        &[&photo, &keyword],
    )? > 0)
}

/// Gives every photo of `ids` `keyword` as the user applied it, and when one
/// did not have it, makes it the most recently applied keyword, as Lightroom
/// stamps its keywords' `lastApplied`.
fn apply(w: &mut Write<'_>, ids: &[PhotoId], keyword: i64) -> Result<()> {
    let mut any = false;
    for id in ids {
        any |= tag_photo(w, *id, keyword)?;
    }
    if any {
        super::keywords::stamp_applied(w, keyword)?;
    }
    Ok(())
}

/// The keyword at `path`, made where missing. A top-level name is looked
/// up both beside and under Lightroom's root (see `keyword`), and a new
/// top-level keyword goes under the root where there is one, as Lightroom's
/// do.
pub(super) fn keyword_at(w: &mut Write<'_>, path: &[String]) -> Result<i64> {
    row! {
        struct Named {
            id: i64,
            name: String,
        }
    }
    let root: Option<i64> = w.read_optional(
        sql!("SELECT id FROM keywords WHERE name='' AND parent IS NULL ORDER BY id LIMIT 1"),
        &[],
    )?;
    let mut parent: Option<i64> = None;
    for name in path {
        let name = keyword_name(name);
        ensure!(!name.is_empty(), "A keyword needs a name");
        // Not unique: the first of any duplicates Lightroom left.
        let candidates: Vec<Named> = match parent {
            Some(parent) => w.read(
                sql!("SELECT id, name FROM keywords WHERE parent=? ORDER BY id"),
                &[&parent],
            )?,
            None => w.read(
                sql!(
                    "SELECT id, name FROM keywords WHERE parent IS NULL
                     OR parent IN (SELECT id FROM keywords WHERE name='' AND parent IS NULL)
                     ORDER BY id"
                ),
                &[],
            )?,
        };
        let found = candidates
            .into_iter()
            .find(|k| keyword_name(&k.name) == name)
            .map(|k| k.id);
        parent = Some(match found {
            Some(id) => id,
            None => super::keywords::new_keyword(w, &name, parent.or(root))?,
        });
    }
    Ok(parent.unwrap())
}

/// A keyword with its path from the top. Lightroom's hidden root, a
/// nameless keyword with no parent that its top-level keywords are under,
/// is no part of a path; the root itself has an empty one.
fn keyword(db: &impl Reads, id: i64) -> Result<Keyword> {
    row! {
        struct Step {
            name: String,
            parent: Option<i64>,
            included: bool,
            with_parents: bool,
        }
    }
    let mut path = Vec::new();
    // Include on Export of each, from the keyword up.
    let mut include = Vec::new();
    let mut parents = true;
    let mut next = Some(id);
    while let Some(k) = next {
        // A cycle in a damaged catalog would never end.
        ensure!(path.len() < 256, "Keyword hierarchy too deep");
        let step: Step = db
            .read_optional(
                sql!(
                    "SELECT k.name, k.parent, COALESCE(e.include, 1), COALESCE(e.parents, 1)
                     FROM keywords k LEFT JOIN keyword_export e ON e.keyword = k.id WHERE k.id=?"
                ),
                &[&k],
            )?
            .context("Unknown keyword")?;
        if step.name.is_empty() && step.parent.is_none() {
            break;
        }
        if path.is_empty() {
            parents = step.with_parents;
        }
        path.push(step.name);
        include.push(step.included);
        next = step.parent;
    }
    let own = include.first().copied().unwrap_or(true);
    let mut exported: Vec<bool> = include
        .iter()
        .enumerate()
        .map(|(i, included)| own && *included && (i == 0 || parents))
        .collect();
    path.reverse();
    exported.reverse();
    Ok(Keyword {
        id,
        name: path.last().cloned().unwrap_or_default(),
        path,
        exported,
    })
}

pub(super) fn read(db: &impl Reads, id: PhotoId) -> Result<Descriptive> {
    row! {
        struct State {
            field: String,
            state: String,
        }
    }
    row! {
        struct Lang {
            lang: String,
            value: String,
        }
    }
    let states: HashMap<String, String> = db
        .read::<State>(
            sql!("SELECT field, state FROM photo_fields WHERE photo=?"),
            &[&id],
        )?
        .into_iter()
        .map(|s| (s.field, s.state))
        .collect();
    let mut d = Descriptive::default();
    for field in TextField::ALL {
        *d.text_mut(field) = match states.get(key(field)).map(String::as_str) {
            Some("set") => {
                let langs: Vec<Lang> = db.read(
                    sql!(
                        "SELECT lang, value FROM photo_text WHERE photo=? AND field=? ORDER BY position"
                    ),
                    &[&id, &key(field)],
                )?;
                Some(Value::Set(LangAlt(
                    langs.into_iter().map(|l| (l.lang, l.value)).collect(),
                )))
            }
            Some(_) => Some(Value::Cleared),
            None => None,
        };
    }
    d.creator = match states.get(CREATOR).map(String::as_str) {
        Some("set") => Some(Value::Set(db.read(
            sql!("SELECT name FROM photo_creators WHERE photo=? ORDER BY position"),
            &[&id],
        )?)),
        Some(_) => Some(Value::Cleared),
        None => None,
    };
    row! {
        struct CaptureRow {
            captured: String,
            subsec: Option<String>,
            offset: Option<String>,
        }
    }
    d.capture = db
        .read_optional::<CaptureRow>(
            sql!(r#"SELECT captured, subsec, "offset" FROM photo_capture WHERE photo=?"#),
            &[&id],
        )?
        .map(|c| Capture {
            captured: c.captured,
            subsec: c.subsec,
            offset: c.offset,
        });
    row! {
        struct LocationRow {
            lat: Option<f64>,
            lon: Option<f64>,
            alt: Option<f64>,
            cleared: bool,
        }
    }
    d.location = db
        .read_optional::<LocationRow>(
            sql!("SELECT lat, lon, alt, cleared FROM photo_location WHERE photo=?"),
            &[&id],
        )?
        .map(|l| match (l.lat, l.lon) {
            (Some(lat), Some(lon)) if !l.cleared => Location::At {
                lat,
                lon,
                alt: l.alt,
            },
            _ => Location::Cleared,
        });
    Ok(d)
}

/// Replaces every descriptive row of a photo with `d`.
pub(super) fn write(w: &mut Write<'_>, id: PhotoId, d: &Descriptive) -> Result<()> {
    for delete in DELETE_ROWS {
        w.execute(delete, &[&id])?;
    }
    let state = |w: &mut Write<'_>, field: &str, set: bool| -> Result<()> {
        w.execute(
            sql!("INSERT INTO photo_fields(photo, field, state) VALUES (?, ?, ?)"),
            &[&id, &field, &if set { "set" } else { "cleared" }],
        )?;
        Ok(())
    };
    for field in TextField::ALL {
        match d.text(field) {
            None => {}
            Some(Value::Cleared) => state(w, key(field), false)?,
            Some(Value::Set(langs)) => {
                state(w, key(field), true)?;
                for (position, (lang, value)) in langs.0.iter().enumerate() {
                    w.execute(
                        sql!(
                            "INSERT INTO photo_text(photo, field, lang, position, value)
                             VALUES (?, ?, ?, ?, ?)
                             ON CONFLICT(photo, field, lang)
                             DO UPDATE SET position=excluded.position, value=excluded.value"
                        ),
                        &[&id, &key(field), lang, &(position as i64), value],
                    )?;
                }
            }
        }
    }
    match &d.creator {
        None => {}
        Some(Value::Cleared) => state(w, CREATOR, false)?,
        Some(Value::Set(names)) => {
            state(w, CREATOR, true)?;
            for (position, name) in names.iter().enumerate() {
                w.execute(
                    sql!("INSERT INTO photo_creators(photo, position, name) VALUES (?, ?, ?)"),
                    &[&id, &(position as i64), name],
                )?;
            }
        }
    }
    if let Some(c) = &d.capture {
        w.execute(
            sql!(r#"INSERT INTO photo_capture(photo, captured, subsec, "offset") VALUES (?, ?, ?, ?)"#),
            &[&id, &c.captured, &c.subsec, &c.offset],
        )?;
        let sort = crate::exif::lightroom_time(&c.captured, c.subsec.as_deref())
            .context("Invalid capture time")?;
        w.execute(
            sql!("UPDATE photos SET captured=? WHERE id=?"),
            &[&sort, &id],
        )?;
    }
    match &d.location {
        None => {}
        Some(Location::Cleared) => {
            w.execute(
                sql!("INSERT INTO photo_location(photo, cleared) VALUES (?, 1)"),
                &[&id],
            )?;
        }
        Some(Location::At { lat, lon, alt }) => {
            w.execute(
                sql!(
                    "INSERT INTO photo_location(photo, lat, lon, alt, cleared) VALUES (?, ?, ?, ?, 0)"
                ),
                &[&id, lat, lon, alt],
            )?;
        }
    }
    Ok(())
}

/// Gives `copy` its own copies of `photo`'s descriptive rows.
pub(super) fn copy_rows(w: &mut Write<'_>, photo: PhotoId, copy: PhotoId) -> Result<()> {
    for statement in [
        sql!(
            "INSERT INTO photo_fields(photo, field, state)
             SELECT ?, field, state FROM photo_fields WHERE photo=?"
        ),
        sql!(
            "INSERT INTO photo_text(photo, field, lang, position, value)
             SELECT ?, field, lang, position, value FROM photo_text WHERE photo=?"
        ),
        sql!(
            "INSERT INTO photo_creators(photo, position, name)
             SELECT ?, position, name FROM photo_creators WHERE photo=?"
        ),
        sql!(
            r#"INSERT INTO photo_capture(photo, captured, subsec, "offset")
               SELECT ?, captured, subsec, "offset" FROM photo_capture WHERE photo=?"#
        ),
        sql!(
            "INSERT INTO photo_location(photo, lat, lon, alt, cleared)
             SELECT ?, lat, lon, alt, cleared FROM photo_location WHERE photo=?"
        ),
    ] {
        w.execute(statement, &[&copy, &photo])?;
    }
    Ok(())
}

/// Removes every descriptive row of a photo, one statement per table.
pub(super) const DELETE_ROWS: [Sql; 5] = [
    sql!("DELETE FROM photo_fields WHERE photo=?"),
    sql!("DELETE FROM photo_text WHERE photo=?"),
    sql!("DELETE FROM photo_creators WHERE photo=?"),
    sql!("DELETE FROM photo_capture WHERE photo=?"),
    sql!("DELETE FROM photo_location WHERE photo=?"),
];

/// The descriptive tables.
#[cfg(test)]
pub(super) const TABLES: [&str; 5] = [
    "photo_fields",
    "photo_text",
    "photo_creators",
    "photo_capture",
    "photo_location",
];
